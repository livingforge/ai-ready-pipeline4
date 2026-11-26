//! Format adapters for the reviewed document workflow.
//!
//! The existing mapping contract uses sheets/cells. For text formats these are
//! logical text containers, not physical spreadsheet coordinates: Word and
//! PowerPoint lay paragraphs and table rows out as rows and table columns as
//! columns, and PDF lays the lines and tables of a page out as rows and
//! columns from where the page draws them. `part` identifies the original
//! container.
//! Native text carries original byte/line positions within the hashed revision;
//! its derived views are read-only and edits happen in the original file.
use crate::{data::*, excel::Workbook};
use anyhow::{Context, Result, bail, ensure};
use lopdf::{Document as Pdf, Object, Stream, content::Content};
use roxmltree::{Document, Node};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub mod encryption;
mod html;
mod html_parse;
mod package;
pub(crate) mod page_layout;
mod pdf_figures;
mod pdf_images;
mod pdf_tags;
mod slide_drawings;
mod slide_fonts;
mod slide_shapes;
mod slide_text;
mod slides;
mod word;
mod word_fonts;

pub use html::BlockShape;
pub(crate) use package::{office_archive, office_package, read_entry, uncompressed_size};
pub use page_layout::Piece as PdfPiece;
pub use slide_shapes::{
    ConnectorEnd, ShapeEdit, ShapeOperation, ShapeProperties, is_shape_operation,
    parse_shape_operations,
};
pub use slides::{
    SlideOperation, SlidePosition, is_slide_operation, parse_slide_operations, slide_order,
    slide_view,
};

/// Whether `value` is an operation on PowerPoint slides or their shapes.
pub fn is_presentation_operation(value: &Value) -> bool {
    is_slide_operation(value) || is_shape_operation(value)
}

const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const MATH: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PRESENTATION: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PACKAGE_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const MARKUP_COMPATIBILITY: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const STRICT_REL: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_PAGE: usize = 32 * 1024 * 1024;

/// Excel packages read as workbooks; templates share the workbook body.
pub const EXCEL_FORMATS: &[&str] = &["xlsx", "xlsm", "xltx", "xltm"];
/// Word packages sharing the WordprocessingML body. VBA and template parts are
/// never read and are copied unchanged on writeback.
pub const WORD_FORMATS: &[&str] = &["docx", "docm", "dotx", "dotm"];
/// PowerPoint packages sharing the PresentationML body: macro-enabled
/// presentations and templates too. VBA and template parts are never read and
/// are copied unchanged on writeback.
pub const POWERPOINT_FORMATS: &[&str] = &["pptx", "pptm", "potx", "potm"];
const NATIVE_FORMATS: &[&str] = &["txt", "md", "csv", "tsv"];
/// HTML pages, laid out as Word text is and written back in their encoding.
pub const HTML_FORMATS: &[&str] = &["html", "htm"];

/// Formats whose extraction supports document structure interpretation.
pub fn structure_formats() -> Vec<&'static str> {
    [
        EXCEL_FORMATS,
        WORD_FORMATS,
        POWERPOINT_FORMATS,
        &["pdf"],
        HTML_FORMATS,
    ]
    .concat()
}

pub fn input_formats() -> Vec<&'static str> {
    [structure_formats().as_slice(), NATIVE_FORMATS].concat()
}

pub fn is_excel(format: &str) -> bool {
    EXCEL_FORMATS.contains(&format)
}

/// Whether `format` (a lowercase extension) is a PowerPoint package's.
pub fn is_powerpoint(format: &str) -> bool {
    POWERPOINT_FORMATS.contains(&format)
}

/// The Open XML formats to resave a binary Office file as, which ARP does not parse.
pub fn binary_office_replacement(format: &str) -> Option<&'static str> {
    match format {
        "xls" | "xlsb" | "xlt" => Some(".xlsx/.xlsm"),
        "doc" | "dot" => Some(".docx/.docm"),
        "ppt" | "pot" | "pps" => Some(".pptx/.pptm"),
        _ => None,
    }
}

pub enum Source {
    Excel(Workbook),
    Text(TextSource),
}

pub struct TextSource {
    raw: Vec<u8>,
    /// The sheets of the extraction, their cells typed.
    extraction: Vec<crate::extraction::Sheet>,
    /// [`TextSource::sheets`], made on first use.
    json: std::sync::OnceLock<Vec<Value>>,
    format: String,
    backend: TextBackend,
    /// The slide layouts of a presentation (see [`slides::layouts`]).
    layouts: Vec<Value>,
    /// The pictures of a presentation's slides, or those an HTML page holds
    /// as data URLs, by asset name.
    images: BTreeMap<String, Vec<u8>>,
    /// The picture files an HTML page shows, by asset name, read on import.
    linked: BTreeMap<String, std::path::PathBuf>,
}

/// The image files an original holds, by asset name; for a PDF, whose
/// figures drawn with lines and areas are among them as SVG pictures, the
/// pages (from 1) that draw each, a note naming the pages with images that
/// could not be made files, and how many images and figures of each page
/// were left out for their shape.
#[derive(Default)]
pub struct ImageAssets {
    pub files: BTreeMap<String, Vec<u8>>,
    pub pages: BTreeMap<String, Vec<u32>>,
    pub note: String,
    pub slender: BTreeMap<u32, usize>,
}

enum TextBackend {
    Office(BTreeMap<String, Vec<u8>>),
    /// `unparsed` lists pages whose content stream could not be read to its end;
    /// their text after that point is missing and they are not written back.
    /// `left_out` lists pages with text in fonts whose codes cannot be read
    /// as characters, with those fonts; that text is missing, and kept as it
    /// is when the page is written back.
    /// `repaired` says the file's cross-reference table did not lead to its
    /// pages, and its objects were found by reading it through.
    Pdf {
        doc: Box<Pdf>,
        unparsed: Vec<u32>,
        left_out: Vec<(u32, BTreeSet<String>)>,
        repaired: bool,
    },
    /// An HTML page, with the encoding its text is read in.
    Html(&'static encoding_rs::Encoding),
    Native,
}

impl Source {
    /// How browsers set off each block of an HTML page beyond its font, by
    /// its address, read from the page again; none for other originals.
    pub fn html_shapes(&self) -> Result<Option<BTreeMap<String, BlockShape>>> {
        let Self::Text(doc) = self else {
            return Ok(None);
        };
        if !html(&doc.format) {
            return Ok(None);
        }
        let decoded = html_parse::decode(&doc.raw)?;
        let layout = html::layout(&html_parse::parse(&decoded.text), &decoded.text)?;
        Ok(Some(layout.shapes()))
    }

    /// Images in Word's media folder, PowerPoint's pictures, the pictures an
    /// HTML page shows and the images a PDF's pages draw are kept as
    /// independently reviewable assets, as are the figures a PDF's pages
    /// draw with lines and areas. Their bytes are read from the original,
    /// or the files an HTML page links, only on import. `limits` says which
    /// of a PDF's images and figures are left out.
    pub fn image_assets(&self, limits: &crate::project::AssetLimits) -> Result<ImageAssets> {
        let files = |files| ImageAssets {
            files,
            ..ImageAssets::default()
        };
        let doc = match self {
            Self::Excel(book) => {
                return Ok(files(
                    book.other_images
                        .iter()
                        .map(|(name, part)| (name.clone(), book.parts[part].clone()))
                        .collect(),
                ));
            }
            Self::Text(doc) => doc,
        };
        if let TextBackend::Pdf { doc, .. } = &doc.backend {
            let (mut images, placed) = pdf_images::read(doc, MAX_PAGE, limits)?;
            pdf_figures::read(doc, &mut images, placed, limits)?;
            if let Some(most) = limits
                .max_aspect_ratio
                .filter(|_| !images.slender.is_empty())
            {
                let pages = images
                    .slender
                    .iter()
                    .map(|(page, count)| format!("{page}ページ（{count}件）"))
                    .collect::<Vec<_>>()
                    .join("、");
                images.note.push_str(&format!(
                    "次のページの画像・図は、長辺が短辺の{most}倍を超えるため、罫線や帯などの飾りとみなしてassetsに保存していません: {pages}。保存するには .arp/config.yml の assets.max_aspect_ratio を大きくしてください（nullにすると縦横比では除外しません）。"
                ));
            }
            return Ok(images);
        }
        if is_powerpoint(&doc.format) {
            return Ok(files(doc.images.clone()));
        }
        if html(&doc.format) {
            let mut images = doc.images.clone();
            for (name, file) in &doc.linked {
                ensure!(
                    fs::metadata(file)?.len() <= 256 * 1024 * 1024,
                    "picture {} exceeds size budget",
                    file.display()
                );
                images.insert(
                    name.clone(),
                    fs::read(file).with_context(|| format!("read {}", file.display()))?,
                );
            }
            return Ok(files(images));
        }
        if !word(&doc.format) {
            return Ok(ImageAssets::default());
        }
        // The images of the media folder and those the document's parts
        // name elsewhere, but not an SVG drawn with the PNG beside it that
        // Word draws of it, which is the same picture.
        let (wanted, drawn_twice) = match &doc.backend {
            TextBackend::Office(parts) => word_images(parts),
            _ => Default::default(),
        };
        let mut archive = office_archive(&doc.raw)?;
        let mut images = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name();
            if entry.is_dir()
                || drawn_twice.contains(name)
                || !(name.starts_with("word/media/") || wanted.contains(name))
            {
                continue;
            }
            ensure!(
                entry.size() <= 256 * 1024 * 1024,
                "Word image exceeds size budget"
            );
            let bytes = read_entry(&mut entry)?;
            images.insert(entry.name().to_owned(), bytes);
        }
        // Word keeps a copy of a picture for older readers (the VML of an
        // `mc:Fallback`): the same bytes are one picture.
        let mut named = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for (part, bytes) in images {
            if !seen.insert(hash(&bytes)) {
                continue;
            }
            let extension = Path::new(&part)
                .extension()
                .and_then(|s| s.to_str())
                .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or("bin");
            named.insert(format!("image-{:03}.{extension}", seen.len()), bytes);
        }
        Ok(files(named))
    }

    /// The pictures a document shows that are not kept as assets: those
    /// linked to files outside it (by sheet or slide and name, or for Word by
    /// file name), and the pictures of an HTML page that could not be read
    /// (by `src`).
    fn unkept_pictures(&self) -> (Vec<String>, Vec<String>) {
        let mut linked = vec![];
        let mut unread = vec![];
        let drawn = |sheets: &mut dyn Iterator<Item = (&str, &Value)>, linked: &mut Vec<String>| {
            for (sheet, drawings) in sheets {
                for drawing in drawings.as_array().into_iter().flatten() {
                    if drawing["linked_image"].is_string() && drawing["image"].is_null() {
                        let name = drawing["name"].as_str().unwrap_or("");
                        linked.push(format!("{sheet}の「{name}」"));
                    }
                }
            }
        };
        match self {
            Self::Excel(book) => drawn(
                &mut book
                    .sheets
                    .iter()
                    .map(|s| (s["name"].as_str().unwrap_or(""), &s["drawings"])),
                &mut linked,
            ),
            Self::Text(doc) if is_powerpoint(&doc.format) => drawn(
                &mut doc
                    .extraction
                    .iter()
                    .map(|s| (s["name"].as_str().unwrap_or(""), &s["drawings"])),
                &mut linked,
            ),
            Self::Text(doc) if html(&doc.format) => {
                for sheet in &doc.extraction {
                    for image in sheet["images"].as_array().into_iter().flatten() {
                        if image["asset"].is_null()
                            && let Some(src) = image["src"].as_str()
                        {
                            // A data URL is named by its kind, not its bytes.
                            let shown = if src.trim_start().starts_with("data:") {
                                src.split(',').next().unwrap_or("data:").to_owned()
                            } else {
                                src.to_owned()
                            };
                            unread.push(shown);
                        }
                    }
                }
            }
            Self::Text(TextSource {
                format,
                backend: TextBackend::Office(parts),
                ..
            }) if word(format) => linked = word_linked_images(parts),
            _ => {}
        }
        (linked, unread)
    }

    /// Word field instructions are read-only metadata, separate from the
    /// displayed, possibly stale field results in the extraction cells.
    pub fn word_field_codes(&self) -> Result<Vec<Value>> {
        let Self::Text(doc) = self else {
            return Ok(vec![]);
        };
        if !word(&doc.format) {
            return Ok(vec![]);
        }
        let TextBackend::Office(parts) = &doc.backend else {
            return Ok(vec![]);
        };
        let mut result = Vec::new();
        for sheet in &doc.extraction {
            let part = string(&sheet["part"])?;
            let (text, _) = part_text(parts.get(part).context("missing Word part")?)?;
            result.extend(word::extract_field_codes(&Document::parse(&text)?, part));
        }
        Ok(result)
    }

    /// Print-related Open XML elements with their source part. Keep the XML
    /// fragment so settings not modeled by ARP remain visible and auditable.
    pub fn print_settings(&self) -> Result<Vec<Value>> {
        /// The elements of a worksheet holding its print settings.
        const SHEET_PRINT_SETTINGS: [&str; 7] = [
            "printOptions",
            "pageMargins",
            "pageSetup",
            "headerFooter",
            "rowBreaks",
            "colBreaks",
            "pageSetUpPr",
        ];
        let mut result = Vec::new();
        let parts = match self {
            Self::Excel(book) => &book.parts,
            Self::Text(doc) if word(&doc.format) => {
                let TextBackend::Office(parts) = &doc.backend else {
                    return Ok(result);
                };
                parts
            }
            _ => return Ok(result),
        };
        for (part, bytes) in parts {
            let excel_workbook = part == "xl/workbook.xml";
            let excel_sheet = part.starts_with("xl/worksheets/") && part.ends_with(".xml");
            let word_part = part.starts_with("word/") && part.ends_with(".xml");
            if !(excel_workbook || excel_sheet || word_part) {
                continue;
            }
            let (text, _) = part_text(bytes)?;
            let mut collect = |nodes: &mut dyn Iterator<Item = Node<'_, '_>>| {
                for node in nodes.filter(Node::is_element) {
                    let name = node.tag_name().name();
                    let relevant = if excel_workbook {
                        name == "definedName"
                            && matches!(
                                node.attribute("name"),
                                Some("_xlnm.Print_Area" | "_xlnm.Print_Titles")
                            )
                    } else if excel_sheet {
                        SHEET_PRINT_SETTINGS.contains(&name)
                    } else {
                        name == "sectPr" && node.tag_name().namespace() == Some(WORD)
                    };
                    if relevant {
                        let mut xml = String::new();
                        canonical_xml(node, &mut xml);
                        result.push(json!({"part":part,"component":name,"xml":xml}));
                    }
                }
            };
            // A worksheet is read without its cells, as importing it does,
            // and only the parts of its rows naming a print setting.
            let cells = if excel_sheet {
                crate::excel::SheetData::of(&text)?
            } else {
                None
            };
            let Some(cells) = cells else {
                collect(&mut Document::parse(&text)?.descendants());
                continue;
            };
            let rest = cells.rest(&text);
            let xml = Document::parse(&rest)?;
            collect(&mut xml.descendants().filter(|n| n.range().start < cells.start));
            for rows in cells.parts(&text) {
                if SHEET_PRINT_SETTINGS.iter().any(|name| rows.contains(name)) {
                    let rows = Document::parse(&rows)?;
                    let data = rows
                        .root_element()
                        .first_element_child()
                        .context("worksheet rows missing")?;
                    collect(&mut data.descendants().skip(1));
                }
            }
            collect(&mut xml.descendants().filter(|n| n.range().start > cells.start));
        }
        Ok(result)
    }

    /// Expose complete chart XML and its source formulas for read-only review,
    /// those of a workbook and of a presentation's slides. Chart editing
    /// continues through Excel and PowerPoint.
    pub fn chart_parts(&self) -> Result<Vec<Value>> {
        let (parts, folder) = match self {
            Self::Excel(book) => (&book.parts, "xl/charts/"),
            Self::Text(TextSource {
                format,
                backend: TextBackend::Office(parts),
                ..
            }) if is_powerpoint(format) => (parts, "ppt/charts/"),
            Self::Text(TextSource {
                format,
                backend: TextBackend::Office(parts),
                ..
            }) if word(format) => (parts, "word/charts/"),
            _ => return Ok(vec![]),
        };
        let mut result = Vec::new();
        for (part, bytes) in parts {
            if !part.starts_with(folder) || !part.ends_with(".xml") {
                continue;
            }
            let (text, _) = part_text(bytes)?;
            let xml = Document::parse(&text)?;
            let formulas: Vec<_> = xml
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "f")
                .filter_map(|n| n.text().map(str::to_owned))
                .collect();
            result.push(json!({"part":part,"formulas":formulas,"xml":text.as_ref()}));
        }
        Ok(result)
    }

    /// Binary controls and printer settings cannot be interpreted safely, but
    /// their identity and hash are part of the source fidelity record.
    pub fn opaque_parts(&self) -> Vec<Value> {
        let Self::Excel(book) = self else {
            return vec![];
        };
        book.parts
            .iter()
            .filter_map(|(part, bytes)| {
                let lower = part.to_ascii_lowercase();
                let kind = if lower.starts_with("xl/activex/") {
                    "activex"
                } else if lower.starts_with("xl/printersettings/") {
                    "printer_settings"
                } else {
                    return None;
                };
                Some(json!({"part":part,"kind":kind,"sha256":hash(bytes)}))
            })
            .collect()
    }

    /// Whether the original at `path` is a workbook with a VBA project, as
    /// [`Source::vba_report`] of it is some: read from the names of its parts
    /// alone, without parsing the workbook.
    pub fn has_vba_project(path: &Path) -> Result<bool> {
        let format = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !is_excel(&format) {
            return Ok(false);
        }
        let raw = fs::read(path)?;
        let package = office_package(&raw)?;
        let archive = zip::ZipArchive::new(std::io::Cursor::new(&*package))?;
        Ok(archive
            .file_names()
            .any(|name| name.to_ascii_lowercase().ends_with("vbaproject.bin")))
    }

    pub fn open(path: &Path) -> Result<Self> {
        let format = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !is_excel(&format) {
            ensure!(
                input_formats().contains(&format.as_str()),
                "unsupported document format: {format}"
            );
            let size = fs::metadata(path)?.len();
            ensure!(size <= MAX_BYTES as u64, "document exceeds size budget");
            if matches!(format.as_str(), "txt" | "md" | "csv" | "tsv") || html(&format) {
                ensure!(
                    size <= MAX_PAGE as u64,
                    "text document exceeds 32 MiB size budget"
                );
            }
        }
        let raw = fs::read(path)?;
        Self::from_bytes(path, raw)
    }

    /// Parse bytes already read while checking whether an original changed.
    pub(crate) fn from_bytes(path: &Path, raw: Vec<u8>) -> Result<Self> {
        let format = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if is_excel(&format) {
            return Ok(Self::Excel(Workbook::from_bytes(raw)?));
        }
        ensure!(
            input_formats().contains(&format.as_str()),
            "unsupported document format: {format}"
        );
        ensure!(raw.len() <= MAX_BYTES, "document exceeds size budget");
        let native = matches!(format.as_str(), "txt" | "md" | "csv" | "tsv");
        if native || html(&format) {
            ensure!(
                raw.len() <= MAX_PAGE,
                "text document exceeds 32 MiB size budget"
            );
        }
        // A presentation's pictures, which the XML parts leave out.
        let media = if is_powerpoint(&format) {
            slide_drawings::media(&raw)?
        } else {
            BTreeMap::new()
        };
        let mut pictures = html::Pictures::default();
        let (backend, mut sheets) = if native {
            let text = std::str::from_utf8(&raw)
                .context("text documents require UTF-8 (optional BOM); convert the original explicitly before import")?;
            ensure!(
                !text.contains('\0'),
                "text document contains NUL; binary/UTF-16 input is not supported"
            );
            let sheet = match format.as_str() {
                "md" => crate::native_text::markdown(text)?,
                "csv" => crate::native_text::delimited(text, b',')?,
                "tsv" => crate::native_text::delimited(text, b'\t')?,
                _ => crate::native_text::lines(text)?,
            };
            (TextBackend::Native, vec![sheet])
        } else if html(&format) {
            let decoded = html_parse::decode(&raw)?;
            // The tree is let go before the sheet is made, which needs only
            // the layout.
            let mut layout = html::layout(&html_parse::parse(&decoded.text), &decoded.text)?;
            pictures = layout.assets(path, &crate::project::picture_folders(path)?);
            (
                TextBackend::Html(decoded.encoding),
                vec![html::sheet(&layout, &raw, &decoded)?],
            )
        } else if format == "pdf" {
            let (doc, repaired) = pdf_load(&raw)?;
            if let Some(protected) = pdf_protection(&doc) {
                return Err(protected.into());
            }
            let mut sheets = vec![];
            let mut unparsed = vec![];
            let mut left_out = vec![];
            ensure!(
                doc.get_pages().len() <= 10000,
                "PDF page count exceeds budget"
            );
            let mapped = pdf_mapped_fonts(&doc);
            let mut read = PdfFonts::new(&mapped);
            let tags = pdf_tags::read(&doc);
            for (page, id) in doc.get_pages() {
                let raw_content = doc.get_page_content_with_limit(id, MAX_PAGE)?;
                // The lenient decoder stops quietly at the first token it cannot
                // read; keep what it read but report the page.
                let (mut content, whole) = pdf_content(&raw_content)?;
                if !whole {
                    unparsed.push(page);
                }
                let strings = pdf_text(&doc, id, &mut content, &BTreeMap::new(), None, &mut read)?;
                if !read.left_out.is_empty() {
                    left_out.push((page, std::mem::take(&mut read.left_out)));
                }
                let (layout, ..) = pdf_page_layout(&doc, id, &content, &strings, &tags);
                sheets.push(crate::extraction::Sheet::from_value(
                    pdf_sheet(page, &strings, &layout),
                    sheets.len(),
                )?);
            }
            (
                TextBackend::Pdf {
                    doc: Box::new(doc),
                    unparsed,
                    left_out,
                    repaired,
                },
                sheets,
            )
        } else {
            let parts = office_parts(&raw, false)?;
            let containers = office_containers(&parts, &format)?;
            let styles = word(&format)
                .then(|| word_fonts::WordStyles::new(&parts))
                .transpose()?;
            let mut sheets = vec![];
            let texts = slide_fonts::PresentationTexts::new(&parts);
            let presentation = slide_fonts::PresentationParts::new(&texts);
            for (name, part) in containers {
                let (text, _) = part_text(parts.get(&part).context("missing document part")?)?;
                let xml = Document::parse(&text)?;
                if let Some(styles) = &styles {
                    // office_containers leaves the main part's root to be checked here.
                    ensure!(
                        name != "document" || xml.root_element().has_tag_name((WORD, "document")),
                        "not a supported Word document"
                    );
                    let drawn = word_drawn_texts(&parts, &part);
                    sheets.push(word::sheet(&xml, &name, &part, styles, &drawn)?);
                } else {
                    let fonts = slide_fonts::SlideFonts::new(&parts, &part, &xml, &presentation)?;
                    let layout = slide_text::layout(&xml)?;
                    let mut sheet = layout.sheet(&name, &part, &fonts)?;
                    if xml.root_element().has_tag_name((PRESENTATION, "sld")) {
                        let mut drawings =
                            slide_drawings::drawings(&parts, &media, &part, &xml, &fonts)?;
                        // Each shape names the rows its paragraphs take.
                        let rows = layout.shape_rows();
                        for drawing in &mut drawings {
                            let raw = drawing["id"]
                                .as_str()
                                .and_then(|id| id.rsplit_once('#'))
                                .map(|(_, raw)| raw);
                            if drawing["kind"] == "shape"
                                && let Some(rows) = raw.and_then(|raw| rows.get(raw))
                            {
                                drawing["paragraphs"] =
                                    json!(rows.iter().map(|r| format!("A{r}")).collect::<Vec<_>>());
                            }
                        }
                        sheet["drawings"] = json!(drawings);
                    }
                    // A slide hidden from the show is marked like a hidden sheet.
                    if xml.root_element().attribute("show") == Some("0") {
                        sheet["state"] = json!("hidden");
                    }
                    sheets.push(sheet);
                }
            }
            (TextBackend::Office(parts), sheets)
        };
        ensure!(
            !sheets.is_empty(),
            "document has no pages or text containers"
        );
        let layouts = match &backend {
            TextBackend::Office(parts) if is_powerpoint(&format) => slides::layouts(parts)?,
            _ => vec![],
        };
        // Pictures are named in first-use order across the slides, as Excel's are.
        let mut images = BTreeMap::new();
        let mut names: BTreeMap<String, String> = BTreeMap::new();
        if is_powerpoint(&format) {
            for sheet in &mut sheets {
                for drawing in sheet
                    .fields
                    .get_mut("drawings")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    let (Some(sha), Some(part)) = (
                        drawing["image"]["sha256"].as_str().map(str::to_owned),
                        drawing["image"]["part"].as_str().map(str::to_owned),
                    ) else {
                        continue;
                    };
                    let next = names.len() + 1;
                    let extension = Path::new(&part)
                        .extension()
                        .and_then(|s| s.to_str())
                        .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
                        .unwrap_or("bin")
                        .to_owned();
                    let name = names
                        .entry(sha)
                        .or_insert_with(|| format!("image-{next:03}.{extension}"))
                        .clone();
                    // Each name stands for one picture's bytes, copied once.
                    if !images.contains_key(&name)
                        && let Some(bytes) = media.get(&part)
                    {
                        images.insert(name.clone(), bytes.clone());
                    }
                    drawing["image"]["asset"] = json!(name);
                }
            }
            // The images around the slides' objects (backgrounds, layouts,
            // masters, notes pages) are assets of no object.
            if let TextBackend::Office(parts) = &backend {
                let slides: Vec<String> = sheets
                    .iter()
                    .filter_map(|sheet| sheet["part"].as_str())
                    .filter(|part| part.starts_with("ppt/slides/"))
                    .map(str::to_owned)
                    .collect();
                for part in slide_drawings::surrounding_images(parts, &media, &slides) {
                    let Some(bytes) = media.get(&part).or_else(|| parts.get(&part)) else {
                        continue;
                    };
                    let next = names.len() + 1;
                    let extension = Path::new(&part)
                        .extension()
                        .and_then(|s| s.to_str())
                        .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
                        .unwrap_or("bin")
                        .to_owned();
                    let name = names
                        .entry(hash(bytes))
                        .or_insert_with(|| format!("image-{next:03}.{extension}"))
                        .clone();
                    images.entry(name).or_insert_with(|| bytes.clone());
                }
            }
        }
        images.extend(pictures.embedded);
        Ok(Self::Text(TextSource {
            raw,
            extraction: sheets,
            json: std::sync::OnceLock::new(),
            format,
            backend,
            layouts,
            images,
            linked: pictures.files,
        }))
    }

    /// The slide layouts a new slide can be made from; none outside PowerPoint.
    pub fn slide_layouts(&self) -> &[Value] {
        match self {
            Self::Text(doc) => &doc.layouts,
            Self::Excel(_) => &[],
        }
    }

    pub fn raw(&self) -> &[u8] {
        match self {
            Self::Excel(book) => &book.raw,
            Self::Text(doc) => &doc.raw,
        }
    }

    /// The format of the original, as its extension names it (`xlsx`,
    /// `docx`, `pdf`, …).
    pub fn format(&self) -> &str {
        match self {
            Self::Excel(_) => "xlsx",
            Self::Text(doc) => &doc.format,
        }
    }

    /// The text of the XML part `name` of a Word or PowerPoint original, none
    /// for a part it lacks or another format.
    pub fn office_part(&self, name: &str) -> Option<Cow<'_, str>> {
        match self {
            Self::Text(TextSource {
                backend: TextBackend::Office(parts),
                ..
            }) => xml_part(parts, name).ok(),
            _ => None,
        }
    }

    /// The part the first internal relationship of `part` whose type ends
    /// with `kind` (such as `/slideLayout`) names, in a Word or PowerPoint
    /// original.
    pub fn office_related(&self, part: &str, kind: &str) -> Option<String> {
        let Self::Text(TextSource {
            backend: TextBackend::Office(parts),
            ..
        }) = self
        else {
            return None;
        };
        let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
        let text = xml_part(parts, &format!("{directory}/_rels/{filename}.rels")).ok()?;
        let xml = Document::parse(&text).ok()?;
        let rel = xml.descendants().find(|n| {
            n.has_tag_name((PACKAGE_REL, "Relationship"))
                && n.attribute("Type").is_some_and(|t| t.ends_with(kind))
        })?;
        relationship_target(parts, part, rel).ok()
    }

    /// Each page of a PDF original: its size in points (the crop box, else
    /// the media box), the lines and areas it paints, the images it draws,
    /// each string of its text with its font, where it starts and how wide
    /// it runs, in points from its top left corner, and the strings each
    /// cell of the page's sheet in the extraction is made of. None for
    /// another format.
    pub fn pdf_pages(&self) -> Result<Option<Vec<PdfPage>>> {
        let Self::Text(TextSource {
            backend: TextBackend::Pdf { doc, .. },
            ..
        }) = self
        else {
            return Ok(None);
        };
        let mapped = pdf_mapped_fonts(doc);
        let mut read = PdfFonts::new(&mapped);
        let tags = pdf_tags::read(doc);
        let mut pages = vec![];
        let mut placed = pdf_images::placements(doc, MAX_PAGE)?.pages.into_iter();
        for (_, id) in doc.get_pages() {
            let placements = placed
                .next()
                .map(|(_, placements)| placements)
                .unwrap_or_default();
            pages.push(pdf_page(doc, id, &mut read, &tags, placements)?);
        }
        Ok(Some(pages))
    }

    /// The sheets of an extraction of the original, their cells typed, and
    /// the bytes of the original. The parsed original is let go: its sheets
    /// are moved into the typed ones rather than copied.
    pub fn into_extraction(self) -> Result<(Vec<crate::extraction::Sheet>, Vec<u8>)> {
        match self {
            Self::Excel(book) => Ok((book.extraction_sheets()?, book.raw)),
            Self::Text(doc) => Ok((doc.extraction, doc.raw)),
        }
    }

    /// The sheets as the extraction records them, with the fonts and styles
    /// of their cells in place. A workbook builds them from its typed cells on
    /// each call.
    pub fn sheets(&self) -> Cow<'_, [Value]> {
        match self {
            Self::Excel(book) => Cow::Owned(book.sheet_values()),
            Self::Text(doc) => Cow::Borrowed(doc.sheets()),
        }
    }

    pub fn engine(&self) -> &'static str {
        match self {
            Self::Excel(_) => "xml",
            Self::Text(doc) if matches!(doc.backend, TextBackend::Native) => "text",
            Self::Text(doc) if doc.format == "pdf" => "pdf",
            Self::Text(doc) if html(&doc.format) => "html",
            _ => "xml",
        }
    }

    pub fn parser(&self) -> &'static str {
        match self {
            Self::Excel(_) => "cells/1",
            Self::Text(doc) if matches!(doc.backend, TextBackend::Native) => "native-text/1",
            Self::Text(doc) if word(&doc.format) => "word-blocks/1",
            Self::Text(doc) if is_powerpoint(&doc.format) => "slide-blocks/1",
            Self::Text(doc) if html(&doc.format) => "html-blocks/1",
            Self::Text(_) => "page-layout/1",
        }
    }

    pub fn note(&self) -> String {
        let mut note = self.skipped_note();
        if let Self::Text(TextSource {
            backend: TextBackend::Html(encoding),
            ..
        }) = self
        {
            note.push_str(&format!("原本の文字コードは{}です。", encoding.name()));
            if html_parse::ensure_writable(encoding).is_err() {
                note.push_str("この文字コードの原本は書き戻せません。UTF-8かShift_JISで保存し直して再取込してください。");
            }
        }
        if let Self::Excel(book) = self
            && book.date1904
        {
            note.push_str("このブックは1904年日付系です。日付・時刻のセル値は1904年1月1日を0とするシリアル値で、1900年日付系より1462日小さい値です。");
        }
        if let Self::Excel(book) = self
            && !book.rich_values.is_empty()
        {
            note.push_str(&format!(
                "次のセルはセル内の画像またはリンクされたデータ型（株価・地理など）で、値はセル外に保存されているため抽出していません（セルの値は#VALUE!と記録されます）: {}。",
                book.rich_values.join("、")
            ));
        }
        let (linked, unread) = self.unkept_pictures();
        // The first of a long list, and how many more.
        let listed = |items: &[String]| {
            let mut list = items
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("、");
            if items.len() > 10 {
                list.push_str(&format!("ほか{}件", items.len() - 10));
            }
            list
        };
        if !linked.is_empty() {
            note.push_str(&format!(
                "次の画像は文書の外のファイルへのリンクで、文書に保存されていないため、assetsに保存せずOCRもしていません: {}。画像を文書に埋め込んで保存し直すと取り込めます。",
                listed(&linked)
            ));
        }
        if !unread.is_empty() {
            note.push_str(&format!(
                "次の画像は読めないため、assetsに保存せずOCRもしていません（ファイルがない、取り込み元フォルダとpicture_foldersの外にある、Web上にある、画像の形式でない）: {}。",
                listed(&unread)
            ));
        }
        if let Self::Text(TextSource {
            backend: TextBackend::Pdf { repaired: true, .. },
            ..
        }) = self
        {
            note.push_str("このPDFは相互参照表からページをたどれないため、ファイルを先頭から読んでオブジェクトを探しました。PDFの閲覧ソフトで開いて保存し直すと直ります。");
        }
        if let Self::Text(TextSource {
            backend: TextBackend::Pdf { unparsed, .. },
            ..
        }) = self
            && !unparsed.is_empty()
        {
            let pages = unparsed
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("、");
            note.push_str(&format!("次のページは描画命令を途中までしか解析できず、それ以降の文字を抽出していません。書き戻しもできません: {pages}ページ。"));
        }
        if let Self::Text(TextSource {
            backend: TextBackend::Pdf { left_out, .. },
            ..
        }) = self
            && !left_out.is_empty()
        {
            let pages = left_out
                .iter()
                .map(|(page, fonts)| {
                    format!(
                        "{page}ページ（{}）",
                        fonts.iter().cloned().collect::<Vec<_>>().join("、")
                    )
                })
                .collect::<Vec<_>>()
                .join("、");
            note.push_str(&format!("次のページは、文字コードをUnicodeに変換できない複合フォント（対応するUnicode CMapでも2バイトの文字コードのToUnicodeマップでもないもの）で描かれた文字を抽出していません。ページのほかの文字は抽出し、書き戻しではその文字を元のまま残します: {pages}。この文字を抽出するには、ToUnicodeマップを含めて出力し直したPDFか、OCRで文字を付けたPDFを取り込んでください。"));
        }
        note
    }

    fn skipped_note(&self) -> String {
        let base = self.base_note();
        match self {
            Self::Excel(book) if !book.skipped_sheets.is_empty() => {
                let sheets = book
                    .skipped_sheets
                    .iter()
                    .map(|s| {
                        let kind = match s["kind"].as_str() {
                            Some("chartsheet") => "グラフシート",
                            Some("dialogsheet") => "ダイアログシート",
                            _ => "マクロシート",
                        };
                        format!("{}「{}」", kind, s["name"].as_str().unwrap_or(""))
                    })
                    .collect::<Vec<_>>()
                    .join("、");
                format!(
                    "{base}次のシートはセルを持つワークシートではないため抽出していません: {sheets}。書き戻しでは変更せず原本の部品を保持します。"
                )
            }
            _ => base.to_owned(),
        }
    }

    fn base_note(&self) -> &'static str {
        match self {
            Self::Excel(_) => base_note(EXCEL_FORMATS[0]),
            Self::Text(doc) => base_note(&doc.format),
        }
    }

    /// The VBA project of a workbook and the lines of its code that may name
    /// cells; `None` for other documents and workbooks without one.
    pub fn vba_report(&self) -> Option<Value> {
        match self {
            Self::Excel(book) => book.vba_report(),
            Self::Text(_) => None,
        }
    }

    /// Rejects row/column `operations` this document cannot take, before any
    /// output is written: Excel refuses what it cannot move, and Word and
    /// PowerPoint what they cannot copy or remove (text boxes, section breaks,
    /// vertical merges). `all` holds every recorded operation, so that rows of
    /// a slide a slide operation inserts are checked on the slide it copies.
    pub fn ensure_row_edits_supported(
        &self,
        operations: &[crate::excel::StructuralOperation],
        all: &[Value],
    ) -> Result<()> {
        match self {
            Self::Excel(book) => book.ensure_structural_edits_supported(operations),
            Self::Text(doc) => {
                if operations.is_empty() {
                    return Ok(());
                }
                if html(&doc.format) {
                    let operations: Vec<_> = operations.iter().collect();
                    return html::ensure_rows(&doc.raw, &operations);
                }
                ensure!(
                    word(&doc.format) || is_powerpoint(&doc.format),
                    "row operations apply to Excel, Word, PowerPoint and HTML documents only"
                );
                let TextBackend::Office(parts) = &doc.backend else {
                    bail!("Office document without parts");
                };
                // Read by their names and parts: their cells as JSON took
                // several times the memory of the typed ones.
                let slide_operations = if is_powerpoint(&doc.format) {
                    parse_slide_operations(all, &doc.extraction)?
                } else {
                    vec![]
                };
                let origins = slides::origins(&slide_operations, &doc.extraction)?;
                let sheets: BTreeMap<&str, &crate::extraction::Sheet> = doc
                    .extraction
                    .iter()
                    .map(|sheet| Ok((string(&sheet["name"])?, sheet)))
                    .collect::<Result<_>>()?;
                pages_take_rows(parts, operations, word(&doc.format), |name| {
                    // An inserted slide holds the markup of the slide it copies.
                    let sheet = sheets
                        .get(origins.get(name).map(String::as_str).unwrap_or(name))
                        .with_context(|| format!("{name} is not a page of the document"))?;
                    Ok(string(&sheet["part"])?.to_owned())
                })
            }
        }
    }

    /// [`Source::ensure_row_edits_supported`] of the Word document at `path`,
    /// read only for the parts of its pages: opening it lays out all of its
    /// text, which the checks do not read.
    pub fn ensure_word_row_edits_supported(
        path: &Path,
        operations: &[crate::excel::StructuralOperation],
    ) -> Result<()> {
        if operations.is_empty() {
            return Ok(());
        }
        let format = extension(path);
        ensure!(word(&format), "not a Word document");
        let raw = fs::read(path)?;
        ensure!(raw.len() <= MAX_BYTES, "document exceeds size budget");
        let parts = office_parts(&raw, false)?;
        let pages: BTreeMap<String, String> =
            office_containers(&parts, &format)?.into_iter().collect();
        pages_take_rows(&parts, operations, true, |name| {
            pages
                .get(name)
                .cloned()
                .with_context(|| format!("{name} is not a page of the document"))
        })
    }

    /// Rejects slide `operations` before any output is written: they apply to
    /// PowerPoint only, and a slide another part links to cannot be deleted.
    pub fn ensure_slide_edits_supported(&self, operations: &[Value]) -> Result<()> {
        if !operations.iter().any(is_slide_operation) {
            return Ok(());
        }
        let Self::Text(
            doc @ TextSource {
                format,
                backend: TextBackend::Office(parts),
                ..
            },
        ) = self
        else {
            bail!("slide operations apply to PowerPoint presentations only");
        };
        let sheets = doc.sheets();
        ensure!(
            is_powerpoint(format),
            "slide operations apply to PowerPoint presentations only"
        );
        let operations = parse_slide_operations(operations, sheets)?;
        slides::restructure(parts, sheets, &operations)?;
        Ok(())
    }

    /// Writes the edited document to `output`.
    pub fn patch(&self, output: &Path, edits: &Edits<'_>) -> Result<Value> {
        match self {
            Self::Excel(book) => book.patch_with_fonts(
                output,
                edits.operations,
                edits.changes,
                edits.formulas,
                edits.fonts,
                edits.assets,
            ),
            Self::Text(doc) => {
                ensure!(
                    edits.formulas.is_empty(),
                    "text formats support text edits only; formulas are Excel-only"
                );
                ensure!(
                    edits.assets.is_empty() || is_powerpoint(&doc.format),
                    "images are added to Excel workbooks and PowerPoint slides only"
                );
                // Fonts are written to the original text first, so that copied
                // slides and moved rows take them as they take the text's runs.
                let edited = doc.with_fonts(edits.fonts)?;
                let mut report = edited.as_ref().unwrap_or(doc).patch_with(
                    output,
                    edits.operations,
                    edits.changes,
                    edits.assets,
                )?;
                if edited.is_some() {
                    report["font_changes_verified"] = json!(true);
                }
                Ok(report)
            }
        }
    }

    /// Whether `documents values` can edit fonts of this document.
    pub fn font_edits_supported(&self) -> bool {
        match self {
            Self::Excel(_) => true,
            Self::Text(doc) => word(&doc.format) || is_powerpoint(&doc.format),
        }
    }
}

/// The edits a document writer applies.
pub struct Edits<'a> {
    /// Row, column, slide and image operations (`mappings.operations`).
    pub operations: &'a [Value],
    /// New values of cells and text, and with `shape` of shape text.
    pub changes: &'a [Value],
    /// Excel formula edits (see [`Workbook::patch_with_operations_and_assets`]).
    pub formulas: &'a [Value],
    /// Font edits of original cells and shapes: `sheet`, `cell` or `shape`,
    /// and the font properties `after` sets.
    pub fonts: &'a [Value],
    /// Image assets of image operations by path.
    pub assets: &'a BTreeMap<String, Vec<u8>>,
}

/// The sheet of a page's text strings, each with the font it is drawn in
/// when the format records one.
fn office_parts(raw: &[u8], include_binary: bool) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = office_archive(raw)?;
    ensure!(
        zip.len() <= 10000 && uncompressed_size(&mut zip)? <= MAX_BYTES as u128,
        "Office archive exceeds size budget"
    );
    let mut parts = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_owned();
        ensure!(
            entry.enclosed_name().is_some() && !name.contains('\\'),
            "invalid Office part path"
        );
        // Keep the name for relationship checks, but do not retain media and
        // embedded packages until a slide operation actually needs to copy them.
        let bytes = if include_binary || !office_binary(&name) {
            read_entry(&mut entry)?
        } else {
            vec![]
        };
        ensure!(parts.insert(name, bytes).is_none(), "duplicate Office part");
    }
    Ok(parts)
}

fn office_binary(name: &str) -> bool {
    name.rsplit('.').next().is_some_and(|extension| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "bmp"
                | "tif"
                | "tiff"
                | "emf"
                | "wmf"
                | "bin"
                | "xlsx"
                | "xlsm"
                | "xlsb"
                | "docx"
                | "docm"
                | "pptx"
                | "pptm"
                | "pdf"
                | "mp3"
                | "mp4"
                | "wav"
                | "avi"
        )
    })
}

fn validate_office_binary(raw: &[u8]) -> Result<()> {
    let mut zip = office_archive(raw)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if office_binary(entry.name()) {
            std::io::copy(&mut entry, &mut std::io::sink())?;
        }
    }
    Ok(())
}

/// Encodings an XML part may use; UTF-16 requires a byte order mark (XML 1.0 4.3.3).
#[derive(Clone, Copy)]
enum PartEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

fn part_text(bytes: &[u8]) -> Result<(Cow<'_, str>, PartEncoding)> {
    let utf16 = |body: &[u8], unit: fn([u8; 2]) -> u16| -> Result<String> {
        let (pairs, rest) = body.as_chunks::<2>();
        ensure!(rest.is_empty(), "truncated UTF-16 Office part");
        Ok(String::from_utf16(
            &pairs.iter().map(|pair| unit(*pair)).collect::<Vec<_>>(),
        )?)
    };
    Ok(match bytes {
        [0xFF, 0xFE, body @ ..] => (
            Cow::Owned(utf16(body, u16::from_le_bytes)?),
            PartEncoding::Utf16Le,
        ),
        [0xFE, 0xFF, body @ ..] => (
            Cow::Owned(utf16(body, u16::from_be_bytes)?),
            PartEncoding::Utf16Be,
        ),
        _ => (
            Cow::Borrowed(std::str::from_utf8(
                bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes),
            )?),
            PartEncoding::Utf8,
        ),
    })
}

/// Encodes a patched part like the original. The UTF-8 BOM is optional and dropped.
fn encode_part(text: &str, encoding: PartEncoding) -> Vec<u8> {
    let units = |order: fn(u16) -> [u8; 2], mark: [u8; 2]| -> Vec<u8> {
        mark.into_iter()
            .chain(text.encode_utf16().flat_map(order))
            .collect()
    };
    match encoding {
        PartEncoding::Utf8 => text.as_bytes().to_vec(),
        PartEncoding::Utf16Le => units(u16::to_le_bytes, [0xFF, 0xFE]),
        PartEncoding::Utf16Be => units(u16::to_be_bytes, [0xFE, 0xFF]),
    }
}

fn xml_part<'a>(parts: &'a BTreeMap<String, Vec<u8>>, part: &str) -> Result<Cow<'a, str>> {
    Ok(part_text(
        parts
            .get(part)
            .with_context(|| format!("missing Office part: {part}"))?,
    )?
    .0)
}

fn relation_target(parts: &BTreeMap<String, Vec<u8>>, part: &str, id: &str) -> Result<String> {
    let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
    let relationships = if directory.is_empty() {
        format!("_rels/{filename}.rels")
    } else {
        format!("{directory}/_rels/{filename}.rels")
    };
    let text = xml_part(parts, &relationships)?;
    let xml = Document::parse(&text)?;
    let rel = xml
        .descendants()
        .find(|n| n.has_tag_name((PACKAGE_REL, "Relationship")) && n.attribute("Id") == Some(id))
        .context("missing Office relationship")?;
    relationship_target(parts, part, rel)
}

fn relationship_target(
    parts: &BTreeMap<String, Vec<u8>>,
    part: &str,
    rel: Node<'_, '_>,
) -> Result<String> {
    ensure!(
        rel.attribute("TargetMode") != Some("External"),
        "external document part is not supported"
    );
    let target = rel
        .attribute("Target")
        .context("relationship target missing")?;
    resolve_target(part, target, |name| parts.contains_key(name))
}

/// The part an internal relationship `target` of `part` names; `exists` tells
/// whether a part is in the package.
fn resolve_target(part: &str, target: &str, exists: impl Fn(&str) -> bool) -> Result<String> {
    let directory = part.rsplit_once('/').map_or("", |(directory, _)| directory);
    ensure!(
        !target.contains(['\\', ':', '#', '?']),
        "unsupported Office part target"
    );
    let joined = if target.starts_with('/') {
        target.trim_start_matches('/').to_owned()
    } else if directory.is_empty() {
        target.to_owned()
    } else {
        format!("{directory}/{target}")
    };
    let mut normalized = vec![];
    for component in joined.split('/') {
        match component {
            ".." => {
                ensure!(normalized.pop().is_some(), "Office target escapes package");
            }
            "." | "" => {}
            _ => normalized.push(component),
        }
    }
    let literal = normalized.join("/");
    if exists(&literal) || !literal.contains('%') {
        return Ok(literal);
    }
    // Targets are URIs: tools that write spaced or non-ASCII part names
    // percent-encode them in the target while the ZIP entry holds the name.
    Ok(normalized
        .iter()
        .map(|segment| percent_decode(segment))
        .collect::<Result<Vec<_>>>()?
        .join("/"))
}

fn percent_decode(segment: &str) -> Result<String> {
    let bytes = segment.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let byte = segment
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .context("invalid percent-encoding in Office part target")?;
            output.push(byte);
            i += 3;
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(output)?;
    ensure!(
        !decoded.contains(['/', '\\', '\0']) && decoded != ".." && decoded != ".",
        "unsupported Office part target"
    );
    Ok(decoded)
}

fn office_containers(
    parts: &BTreeMap<String, Vec<u8>>,
    format: &str,
) -> Result<Vec<(String, String)>> {
    let roots_text = xml_part(parts, "_rels/.rels")?;
    let roots = Document::parse(&roots_text)?;
    let main_relationship = |namespace: &str| {
        roots.descendants().find(|n| {
            n.has_tag_name((PACKAGE_REL, "Relationship"))
                && n.attribute("Type")
                    .is_some_and(|v| v == format!("{namespace}/officeDocument"))
        })
    };
    ensure!(
        main_relationship(STRICT_REL).is_none(),
        "Strict Open XML documents are not supported; save the file in Office as a standard Open XML document and import that copy"
    );
    let root = main_relationship(REL).context("Office document relationship missing")?;
    let main = relation_target(
        parts,
        "",
        root.attribute("Id").context("missing relationship id")?,
    )?;
    if word(format) {
        // The main part, as large as the document, is parsed once, where its
        // text is read, and its root checked there.
        let mut result = vec![("document".into(), main.clone())];
        let (directory, filename) = main.rsplit_once('/').unwrap_or(("", &main));
        let rel_path = if directory.is_empty() {
            format!("_rels/{filename}.rels")
        } else {
            format!("{directory}/_rels/{filename}.rels")
        };
        if parts.contains_key(&rel_path) {
            let rels_text = xml_part(parts, &rel_path)?;
            let rels = Document::parse(&rels_text)?;
            for rel in rels
                .descendants()
                .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
            {
                let kind = rel
                    .attribute("Type")
                    .unwrap_or("")
                    .strip_prefix(&format!("{REL}/"))
                    .unwrap_or("");
                if matches!(
                    kind,
                    "header" | "footer" | "footnotes" | "endnotes" | "comments"
                ) {
                    let part = relationship_target(parts, &main, rel)?;
                    if !result.iter().any(|(_, p)| p == &part) {
                        result.push((format!("{kind}-{}", result.len()), part));
                    }
                }
            }
        }
        Ok(result)
    } else {
        let main_text = xml_part(parts, &main)?;
        let xml = Document::parse(&main_text)?;
        ensure!(
            xml.root_element()
                .has_tag_name((PRESENTATION, "presentation")),
            "not a supported presentation"
        );
        let (directory, filename) = main.rsplit_once('/').unwrap_or(("", &main));
        let rel_path = if directory.is_empty() {
            format!("_rels/{filename}.rels")
        } else {
            format!("{directory}/_rels/{filename}.rels")
        };
        let rels_text = xml_part(parts, &rel_path)?;
        let rels = Document::parse(&rels_text)?;
        let mut by_id = BTreeMap::new();
        for rel in rels
            .descendants()
            .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
        {
            if let Some(id) = rel.attribute("Id") {
                by_id.entry(id).or_insert(rel);
            }
        }
        let slides = xml
            .descendants()
            .filter(|n| n.has_tag_name((PRESENTATION, "sldId")))
            .enumerate()
            .map(|(i, n)| {
                Ok((
                    format!("slide-{}", i + 1),
                    relationship_target(
                        parts,
                        &main,
                        *by_id
                            .get(
                                n.attribute((REL, "id"))
                                    .context("missing slide relationship")?,
                            )
                            .context("missing Office relationship")?,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        // Speaker notes follow all slides, so that a slide keeps its page.
        let mut notes = vec![];
        for (i, (_, slide)) in slides.iter().enumerate() {
            let (directory, filename) = slide.rsplit_once('/').unwrap_or(("", slide));
            let rels_path = if directory.is_empty() {
                format!("_rels/{filename}.rels")
            } else {
                format!("{directory}/_rels/{filename}.rels")
            };
            if !parts.contains_key(&rels_path) {
                continue;
            }
            let rels_text = xml_part(parts, &rels_path)?;
            let rels = Document::parse(&rels_text)?;
            if let Some(id) = rels
                .descendants()
                .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
                .find(|n| n.attribute("Type") == Some(&format!("{REL}/notesSlide")))
                .and_then(|n| n.attribute("Id"))
            {
                notes.push((
                    format!("notes-{}", i + 1),
                    relation_target(parts, slide, id)?,
                ));
            }
        }
        Ok(slides.into_iter().chain(notes).collect())
    }
}

/// The note on what the extraction of an original in `format`, its file
/// extension in lower case, reads and leaves out, before what the original
/// itself adds to it.
pub fn base_note(format: &str) -> &'static str {
    match format {
        "md" => {
            "Markdownの見出し階層・段落・リスト・表・コード等を原文のままブロック単位で抽出します。textのA1等はブロック番号です。positionの行範囲・UTF-8バイト範囲と原本ハッシュで出典を確認してください。リンク先・画像は読み込みません。本文は原本を直接編集し、同じ文書IDで再取込してください。確認用YAMLの本文変更・export/applyは未対応です。"
        }
        "csv" | "tsv" => {
            "CSV/TSVを全項目文字列としてレコード・列単位で抽出します。先頭行も通常のレコードで、ヘッダーや型を推測しません。引用符内の改行・区切り文字、先頭ゼロ、空欄を保持します。recordsのA1等は列・レコード番号で、物理行番号ではありません。positionは原本の引用符を含むフィールドの行・UTF-8バイト範囲です。引用符は構文として復号します。原本を直接編集して再取込してください。YAML本文変更・export/applyは未対応です。"
        }
        "txt" => {
            "UTF-8テキストを空行を含め行単位で抽出します。textのA1等は原本の行番号です。positionの行・UTF-8バイト範囲と原本ハッシュで出典を確認してください。原本を直接編集して同じ文書IDで再取込してください。確認用YAMLの本文変更・export/applyは未対応です。"
        }
        format if is_excel(format) => {
            "セル値・数式原文・結合範囲・書式の識別情報・Excelテーブル定義とDrawingMLの図形文字・配置・明示的な接続を抽出します。埋込み画像はassetsに保存し、WindowsではOCRを自動実行して結果を記録します。表・見出しの推定はstructure initで行い、レビューが必要です。グループ内位置は変換情報を保持します。メモ・スレッドコメント、図形に割り当てたマクロ、フォームコントロールの種類・リンク先セル・選択肢範囲も抽出します。接続のない矢印の意味とActiveXコントロールの内部設定は未抽出です。印刷設定とグラフXMLは読み取り専用で抽出し、ActiveX部品はハッシュで記録します。"
        }
        format if html(format) => {
            "HTMLの<title>と本文を、見出し・段落・リスト項目・表のセルなどのブロック単位で抽出します。<title>を先頭行に置き、表の外のブロックは1ブロック1行でA列に並べます。表は行・列の位置を保ち、colspan・rowspanをmerges、表の範囲をtablesに記録します。見出し行はthead、なければthだけの先頭行、合計行はtfootの行です。ブラウザの表示と同じく、theadの行は表の先頭、tfootの行は末尾に置き、rowspanは行グループ（thead・tbody・tfoot）の外へ伸ばしません。表のセルの外に書いた文字は表の前のブロックにします。表を含む表はレイアウト用の枠とみなし、中の段落と表を本文として並べます。空白はブラウザと同じく連続を1つにまとめ、ブロックの先頭・末尾と<br>の前後で除き、preでは原文どおり保ちます。<br>は改行、セル内の段落は改行で表し、ブロック末尾の<br>は表示どおり除きます。空白と&nbsp;だけのブロックは行にしません。pre内のブロック要素（行ごとに要素を置くハイライト済みのコード）は、preのブロックの中の行として改行で区切ります。文字参照は文字に直します。CSSは解釈せず、文字の大きさはブラウザ標準の値（本文12pt、h1 24pt〜h6 8pt）、太字はh1〜h6・th・b・strongから決めます。<main>かrole=\"main\"の要素があればその中だけを本文とし、navとrole=\"navigation\"・\"banner\"・\"contentinfo\"・\"search\"の要素は除きます。head・script・style・template・hidden属性の要素（選ばれていないタブのrole=\"tabpanel\"は除く）・svg・mathの中身、フォーム部品は抽出しません。CSSで非表示にした文字は抽出します。<a href>はlinks（ブロックのセル・url・リンクの文字）、idとaのnameはanchorsに記録します。リンクの行き先になるため、本文から除いた部分やhidden属性の要素のものも、その位置か後のブロックのセルとして記録します（templateの中は除きます）。<img>（遅延読み込みのdata-srcを優先）と、画像の<input>・<object>・<embed>、SVGの中の<image>、style属性の背景画像はimagesにブロックのセル・src・altを記録し（相対パスは<base>からたどります）、data URLの画像と、ページから相対パスで指す画像ファイルのうち取り込み元フォルダと設定のpicture_folders（.arp/config.ymlはプロジェクト内、Git管理外の.arp/local.ymlはこのマシンのプロジェクト外も可）の中にあるものは取込時にassetsへ保存し、その外の画像ファイルは読みません。保存した画像にはWindowsでOCRを自動実行します。画像ファイルは取込時点の内容で、画像だけを変えても原本の変更として検出しません。書き戻しは変更した文字だけを原本の文字コードで書き換え、ほかのバイトは保ちます。改行は<br>として書き、要素をまたぐ変更は最初の要素が新しい文字を受け取ります。表せない文字、HTMLの表示で失われる空白（連続・行頭・行末の空白とタブ）、表のセル内の段落やpre内のブロックをまたぐ変更は拒否します。段落・リスト項目・表の行はdocuments rowsで追加・削除できます。出力をブラウザで開いて表示を確認してください。"
        }
        "pdf" => {
            "ページが描く文字列を、描く位置と罫線から行・列・表に並べて抽出します。同じ行で続く文字列は、描く順や分け方（1文字ずつ等）によらず1つのセルにまとめ、間隔の空く箇所を空白で表します（日本語の文字どうしと、日本語と英数字の間の字間は空白にしません）。交わる罫線は表とし、罫線のないセルどうしは結合としてmerges、表の範囲はtablesに記録します。罫線がなくても、同じ列に2行以上並ぶ文字列は表とします（列をまたぐ文字列のある行と、表の前後の1項目だけの行は表に含めません）。同じ行で文字の大きさの1.8倍以上離れた文字列は別の文字列とし、それより狭い間隔でも、3行以上にわたって縦に通る空白（文字の大きさの0.5倍以上）は列の境目として分けます（箇条書きの記号・番号の後の空白は分けません）。罫線なしの表では、上の行が折り返されている行（上の行が列幅いっぱいまで続く長い文で、文末で終わっていない行）を同じセルにまとめ、折り返しは改行にせずつなぎます。左右の文がそれぞれ別の行で折り返している場合と、どの列も箇条書きである場合は表にせず、左の列から順に読みます。3行以下の表のすぐ下に、同じ行間で一方の列だけが続く場合（長さの違う文が左右に並ぶ場合）も表にしません。2段組みの本文は段ごとに読みます。表の外は1行ずつA列から並べ、同じ行で離れた文字列はB列以降に置き、段組みは列ごとに上から読みます。セル内の行は改行で表します。タグ付きPDFは構造ツリーから段落・見出し・リスト項目・表のセルごとに文字列をまとめ、折り返した行（次の語が入らなかった行）は改行を入れずにつなぎ、余白を残して改めた行は改行で表します。1文字ずつ下へ並ぶ行は縦書きとして1つにつなぎます。段落が次の段へ続くときは段ごとに読んで1つにつなぎます。同じ行に並ぶ段落は、間隔がなくても別々のセルとし、上下の行と重なる位置から列を決めます（Wordは罫線のない表を表としてタグ付けせず、セルを段落として出力します）。構造ツリーが表として示す表は、ページが罫線で描いていればセルが1つずつ罫線の枠に入る限り罫線から読み（結合セルと、ページごとに繰り返す見出し行が分かるため）、罫線がない表と罫線どうしが重なる表は構造ツリーの行・セル・見出し行を使います。結合で省かれたセルのある行は、セルの位置から列を決めます。Excelがシート全体を1つの表としたタグと、入れ子の表は使わず、罫線から読みます。タグが疑わしいと記録されたPDF（Suspects）はタグを使いません。見出し行は、構造ツリーの見出し行、網掛けの先頭行、3列以上の表の先頭行から推定するため、構造のレビューで確認してください。回転した文字列は1つずつ別の行にします。不可視の文字（スキャンページに重ねたOCRの文字）は見える文字と別のセルにします。Form XObjectの中の文字と、注釈の表示の文字（コメント・入力済みのフォーム欄。チェックボックス等のボタンを除く）も描く位置で並べますが、書き戻せません。ページが描く画像（Form XObjectの中のもの、インライン画像、スタンプ・署名など注釈の表示に描く画像、タイリングパターン・ソフトマスク・Type3フォントの字形の中の画像を含む。インライン画像は短い辺が8画素未満か1024画素未満の記号・罫線を除く）は取込時に画像ファイルにしてassetsへ保存し、描くページをpagesに記録して、WindowsでOCRを自動実行します。帯状・格子状に分けて描かれた1枚の画像と、ページのType3フォントの字形の画像は、それぞれ1枚の画像にまとめます。JPEGはそのまま（Decodeで反転するものは解読してPNG）、そのほか（CCITT Fax・JBIG2・JPEG 2000を含む）は解読してPNGにし、ソフトマスク・マスク・JPEG 2000のアルファによる透明度はPNGの透明度として保ちます。OCRは透明な部分を画像と見分けられる背景色の上で読み、OCRの上限より大きい画像は縮小して読みます。色空間を読めない画像と解読できない画像は保存せず、注記にページと理由を記録します。ページが線と塗りで描く図（Word・Excel・PowerPointの図形、グラフ、ブラウザで印刷したSVGなど）は、近くに描かれた線と塗りをまとめて図ごとのSVGにし、figure-001.svgのようにassetsへ保存します。SVGは範囲内の線と塗り、文字（ページのセルごとに1つのtext要素）、保存した画像への参照からなり、viewBoxはページ上の位置（ポイント、左上から）です。文字はocr.textにも記録します。図の範囲は推定です。曲線か斜めの線を含むまとまりと、水平・垂直の線と塗りが3個以上あり表でも枠でもないまとまりを図とし、6ポイント以内に描かれたものを1つの図にまとめます。図に重なる枠や線、画像の上に描いた印（画面の一部を囲む枠など）も図に含め、ページの半分以上を囲む枠は図に含めません。文字のない罫線の格子と、罫線のない表のセルの網掛けは表とみなし、図にしません。中に何も描かれず、隣に箱がなく、辺の罫線が角の先へ続かない箱が3個以上あるまとまりは、シートが表と読んでいても図とします（矢印のない組織図、画面レイアウトなど）。線が辺につながる箱のまとまり（名前の帯と列を重ねたER図のエンティティなど）も図とします。塗りとその輪郭は1個と数え、同じ画像の上の印は1つの図にまとめます。図の線（箱と、箱をつなぐ線）はシートの表にせず、図の中の文字は図の位置に1行ずつ並べます。図の中の罫線は、2列・2行以上で4つ以上のセルに文字がある場合だけ表とし、箱の図では表にしません。表の中に描いた小さな図では、表を表のまま読みます。罫線が線で描かれ、2列・2行以上で4つ以上のセルに文字がある表は、接して描かれた図に含めず、接した図形の箱もその表のセルにしません。罫線の交点どうしを結ぶ斜線は表のセルの斜線として表に含め、短辺が16ポイント未満のものは記号とみなして除きます。表の中の小さな図は図だけを切り出します。グラデーションは始まりの色の単色で塗ります。Form XObjectの中の線と塗り、クリップ・破線・不透明度、横書きでない文字は図に描きません。長辺が短辺の20倍を超える画像と図は、罫線や帯などの飾りとみなして保存せず、注意事項にページと件数を記録します（倍率は.arp/config.ymlのassets.max_aspect_ratioで変更でき、nullにすると縦横比では除外しません）。相互参照表からページをたどれないPDFは、ファイル内のオブジェクトを探して読み、注記に記録します。OCRは読み取った単語の位置と画像の罫線から、ページの文字列と同じ規則で行・列・表に並べ、textに行を改行、セルをタブで区切って読む順に記録し、layoutにセルごとの文字・画像上の位置・結合・表の範囲を記録します。OCRの文字はセルの値にしません。書き戻しは変更した文字を含む文字列だけを書き換え、複数の文字列にまたがる変更は最初の文字列が新しい文字を受け取ります。元フォントで表現可能な文字だけ書き戻せます。文字列の間の空白とセル内の改行は文字ではなく配置から決まるため、それを追加・削除する変更は拒否します。自動改行・再配置はしないため、文字の重なりやはみ出しを出力PDFで確認してください。"
        }
        format if word(format) => {
            "Word本文・表・テキストボックス・ヘッダー・フッター・脚注・文末脚注・コメントを段落と表のセル単位で抽出します。表の外の段落は1段落1行でA列に並び、表は行・列の位置を保ち、結合セルをmerges、表の範囲をtablesに記録します。見出し行はWordの見出し行の繰り返し、太字・網掛けの先頭行、3列以上の表の先頭行から推定するため、構造のレビューで確認してください。セル内の段落・改行は改行、タブはタブ文字で表します。セル内の入れ子の表はそのセルの範囲に独自の行・列を持ち、外側のセルは入れ子の表に合わせて広がり（他のセルは結合として記録）、tablesに外側の表をparentとして記録します。罫線がなく、値のある全セルが表を持つ外側の表はレイアウト用の枠とみなし、表として記録しません。入れ子の表を持つ表の行の追加・削除は拒否します。書き戻しは変更箇所を含むrunだけを書き換えます。段落内の改行・タブは追加・削除・置換でき、runの改行（w:br）・タブ（w:tab）として書き込みます。段落の分割・結合となる変更は拒否します。テキストボックスは表示される1組だけを抽出し、書き戻しでは互換用の複製（VML）も同じ文字にします。画像はassetsに保存し、WindowsではOCRを自動実行します。フィールド命令はfield_codesへ読み取り専用で記録します。フィールドの表示結果（日付・ページ番号・目次・相互参照等）はWordが再計算するため書き戻しを拒否します。ページは再組版されるため出力Wordの表示を確認してください。"
        }
        _ => {
            "PPTXの各スライドの本文・表を段落と表のセル単位で抽出し、スライドのノートはスライドの後にnotes-N（Nはスライド番号）として抽出します。表の外の段落は図形をまたいでスライドの上から1段落1行でA列に並び、表は行・列の位置を保ち、結合セルをmerges、表の範囲をtablesに記録します。セル内の段落は改行で表します。ノートのスライド番号・日付・ヘッダー・フッターはノートマスターから表示されるため抽出しません。書き戻しは変更箇所を含むrunだけを書き換え、段落内の改行・タブを追加・削除・置換できます。段落の分割・結合となる変更は拒否し、段落・表の行はdocuments rowsで追加・削除します。スライドはdocuments slidesで複製・削除でき、複製したスライドは操作IDの名前で本文を編集します。マスターの文字と画像内の文字（OCRはassetsに記録）は本文に含めません。SmartArtの文字とグラフのタイトルは図形のtextに、グラフのXMLはchart_partsに読み取り専用で記録します。自動サイズ調整を行わないため、出力PPTXではみ出しを確認してください。"
        }
    }
}

fn word(format: &str) -> bool {
    WORD_FORMATS.contains(&format)
}

fn html(format: &str) -> bool {
    HTML_FORMATS.contains(&format)
}

/// `text` with its percent-encoded UTF-8 decoded, as a URL path names a
/// file; a sequence that is not one is kept as written.
pub(crate) fn percent_decode_lenient(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(byte) = text
                .get(at + 1..at + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

/// The format of the document at `path`: its extension, in lowercase.
fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// Whether the document at `path` is a Word document.
pub fn is_word_document(path: &Path) -> bool {
    word(&extension(path))
}

/// Rejects the row `operations` that a page cannot take: what Word, when
/// `word_parts`, or PowerPoint cannot copy or remove. `part` names the part
/// of the page `parts` holds.
fn pages_take_rows(
    parts: &BTreeMap<String, Vec<u8>>,
    operations: &[crate::excel::StructuralOperation],
    word_parts: bool,
    part: impl Fn(&str) -> Result<String>,
) -> Result<()> {
    let names: BTreeSet<&str> = operations.iter().map(|o| o.sheet.as_str()).collect();
    for name in names {
        let own: Vec<_> = operations.iter().filter(|o| o.sheet == name).collect();
        let (text, _) = part_text(&parts[&part(name)?])?;
        let xml = Document::parse(&text)?;
        let values = word::InsertedText::new();
        if word_parts {
            let drawn = word_drawn_texts(parts, &part(name)?);
            word::rows_layout(&xml, &drawn)?.restructure(&text, &own, &values)
        } else {
            slide_text::layout(&xml)?.restructure(&text, &own, &values)
        }
        .with_context(|| name.to_owned())?;
    }
    Ok(())
}

/// The text elements, tabs and line breaks that display a field result, keyed
/// by their start in the part. Complex fields show their result between
/// `separate` and `end`; nested fields count as results when any enclosing
/// field has reached its result.
fn word_field_results(xml: &Document<'_>) -> BTreeSet<usize> {
    let mut open: Vec<bool> = vec![];
    let mut results = BTreeSet::new();
    let hidden = HiddenBranches::new(xml, "docx");
    for node in xml.descendants().filter(|n| !hidden.contains(*n)) {
        if node.has_tag_name((WORD, "fldChar")) {
            match node.attribute((WORD, "fldCharType")) {
                Some("begin") => open.push(false),
                Some("separate") => {
                    if let Some(separated) = open.last_mut() {
                        *separated = true;
                    }
                }
                Some("end") => {
                    open.pop();
                }
                _ => {}
            }
        } else if (node.has_tag_name((WORD, "t")) || word::text_break(node))
            && (open.contains(&true)
                || node
                    .ancestors()
                    .any(|n| n.has_tag_name((WORD, "fldSimple"))))
        {
            results.insert(node.range().start);
        }
    }
    results
}

fn text_node(node: Node<'_, '_>, format: &str) -> bool {
    node.has_tag_name((if word(format) { WORD } else { DRAWING }, "t"))
}

const DIAGRAM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
const CHART: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

/// The text a SmartArt graphic shows, from its data part `part`: that of each
/// of its nodes (not of the connections between them), a line each.
pub(crate) fn diagram_text(parts: &BTreeMap<String, Vec<u8>>, part: &str) -> String {
    let read = || -> Option<String> {
        let (text, _) = part_text(parts.get(part)?).ok()?;
        let xml = Document::parse(&text).ok()?;
        let lines: Vec<String> = xml
            .descendants()
            .filter(|n| {
                n.has_tag_name((DIAGRAM, "pt"))
                    && matches!(n.attribute("type"), None | Some("node" | "asst"))
            })
            .flat_map(|point| {
                point
                    .descendants()
                    .filter(|n| {
                        n.tag_name().name() == "p" && n.tag_name().namespace() == Some(DRAWING)
                    })
                    .map(|p| {
                        p.descendants()
                            .filter(|n| n.tag_name().name() == "t")
                            .filter_map(|n| n.text())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|line| !line.is_empty())
            .collect();
        Some(lines.join("\n"))
    };
    read().unwrap_or_default()
}

/// The titles a chart (part `part`) shows: its own and its axes', written
/// or taken from cells, a line each.
pub(crate) fn chart_titles(parts: &BTreeMap<String, Vec<u8>>, part: &str) -> String {
    let read = || -> Option<String> {
        let (text, _) = part_text(parts.get(part)?).ok()?;
        let xml = Document::parse(&text).ok()?;
        let titles: Vec<String> = xml
            .descendants()
            .filter(|n| n.has_tag_name((CHART, "title")))
            .map(|title| {
                let written: Vec<String> = title
                    .descendants()
                    .filter(|n| {
                        n.tag_name().name() == "p" && n.tag_name().namespace() == Some(DRAWING)
                    })
                    .map(|p| {
                        p.descendants()
                            .filter(|n| n.tag_name().name() == "t")
                            .filter_map(|n| n.text())
                            .collect::<String>()
                    })
                    .filter(|line| !line.is_empty())
                    .collect();
                if written.is_empty() {
                    // A title taken from a cell keeps the cell's text.
                    title
                        .descendants()
                        .filter(|n| n.has_tag_name((CHART, "v")))
                        .filter_map(|n| n.text())
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    written.join("\n")
                }
            })
            .filter(|title| !title.is_empty())
            .collect();
        Some(titles.join("\n"))
    };
    read().unwrap_or_default()
}

/// The text the SmartArt graphics and charts of the Word part `part` show,
/// by the relationship naming each's data part (SmartArt) or chart: Word
/// draws it, and shows it in the paragraph the graphic is in.
pub(super) fn word_drawn_texts(
    parts: &BTreeMap<String, Vec<u8>>,
    part: &str,
) -> BTreeMap<String, String> {
    let mut texts = BTreeMap::new();
    let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
    let Some(Ok((rels_text, _))) = parts
        .get(&format!("{directory}/_rels/{filename}.rels"))
        .map(|r| part_text(r))
    else {
        return texts;
    };
    let Ok(rels) = Document::parse(&rels_text) else {
        return texts;
    };
    for rel in rels
        .descendants()
        .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
    {
        let (Some(id), Some(kind)) = (rel.attribute("Id"), rel.attribute("Type")) else {
            continue;
        };
        let Ok(target) = relationship_target(parts, part, rel) else {
            continue;
        };
        let text = if kind.ends_with("/diagramData") {
            diagram_text(parts, &target)
        } else if kind.ends_with("/chart") {
            chart_titles(parts, &target)
        } else {
            continue;
        };
        if !text.is_empty() {
            texts.insert(id.to_owned(), text);
        }
    }
    texts
}

/// The files the pictures of a Word document link to without keeping them
/// (`r:link` with no `r:embed`), by file name, each once.
fn word_linked_images(parts: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut found = vec![];
    for (part, bytes) in parts {
        if !part.starts_with("word/")
            || !part.ends_with(".xml")
            || !bytes.windows(5).any(|w| w == b":link")
        {
            continue;
        }
        let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
        let (Some(Ok((rels_text, _))), Ok((text, _))) = (
            parts
                .get(&format!("{directory}/_rels/{filename}.rels"))
                .map(|r| part_text(r)),
            part_text(bytes),
        ) else {
            continue;
        };
        let (Ok(rels), Ok(xml)) = (Document::parse(&rels_text), Document::parse(&text)) else {
            continue;
        };
        for blip in xml.descendants().filter(|n| {
            n.tag_name().name() == "blip"
                && n.attribute((REL, "link")).is_some()
                && n.attribute((REL, "embed")).is_none()
        }) {
            let id = blip.attribute((REL, "link")).unwrap_or("");
            let Some(target) = rels
                .descendants()
                .find(|n| {
                    n.has_tag_name((PACKAGE_REL, "Relationship")) && n.attribute("Id") == Some(id)
                })
                .and_then(|rel| rel.attribute("Target"))
            else {
                continue;
            };
            let name = percent_decode_lenient(target.rsplit(['/', '\\']).next().unwrap_or(target));
            if !found.contains(&name) {
                found.push(name);
            }
        }
    }
    found
}

/// The image parts the parts of a Word document name (its body, headers,
/// footers, notes and comments), and of those the SVGs a picture shows with
/// a PNG beside it: Word keeps both, drawing the PNG of the SVG for readers
/// without SVG.
fn word_images(parts: &BTreeMap<String, Vec<u8>>) -> (BTreeSet<String>, BTreeSet<String>) {
    const SVG: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";
    let mut named = BTreeSet::new();
    let mut drawn_twice = BTreeSet::new();
    for (part, bytes) in parts {
        if !part.starts_with("word/") || !part.ends_with(".xml") {
            continue;
        }
        let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
        let Some(Ok(rels_text)) = parts
            .get(&format!("{directory}/_rels/{filename}.rels"))
            .map(|r| part_text(r).map(|(text, _)| text))
        else {
            continue;
        };
        let Ok(rels) = Document::parse(&rels_text) else {
            continue;
        };
        let target = |id: &str| {
            rels.descendants()
                .find(|n| {
                    n.has_tag_name((PACKAGE_REL, "Relationship")) && n.attribute("Id") == Some(id)
                })
                .and_then(|rel| relationship_target(parts, part, rel).ok())
        };
        for rel in rels.descendants().filter(|n| {
            n.has_tag_name((PACKAGE_REL, "Relationship"))
                && n.attribute("Type").is_some_and(|t| t.ends_with("/image"))
        }) {
            if let Ok(image) = relationship_target(parts, part, rel) {
                named.insert(image);
            }
        }
        if !bytes.windows(7).any(|w| w == b"svgBlip") {
            continue;
        }
        let Ok((text, _)) = part_text(bytes) else {
            continue;
        };
        let Ok(xml) = Document::parse(&text) else {
            continue;
        };
        for svg in xml
            .descendants()
            .filter(|n| n.has_tag_name((SVG, "svgBlip")))
        {
            let blip = svg
                .ancestors()
                .find(|n| n.tag_name().name() == "blip" && n.attribute((REL, "embed")).is_some());
            if blip.is_some()
                && let Some(image) = svg.attribute((REL, "embed")).and_then(target)
            {
                drawn_twice.insert(image);
            }
        }
    }
    (named, drawn_twice)
}

/// The image a picture's `a:blip` embeds: its own, or the SVG its
/// extension names when the picture is an SVG written with no PNG beside it,
/// as PowerPoint 365 writes one.
pub(crate) fn blip_embed<'a>(blip: Node<'a, '_>) -> Option<&'a str> {
    const SVG: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";
    blip.attribute((REL, "embed")).or_else(|| {
        blip.descendants()
            .find(|n| n.has_tag_name((SVG, "svgBlip")))?
            .attribute((REL, "embed"))
    })
}

fn alternate_branch(node: Node<'_, '_>) -> bool {
    (node.has_tag_name((MARKUP_COMPATIBILITY, "Choice"))
        || node.has_tag_name((MARKUP_COMPATIBILITY, "Fallback")))
        && node
            .parent()
            .is_some_and(|p| p.has_tag_name((MARKUP_COMPATIBILITY, "AlternateContent")))
}

/// Word writes text boxes and shapes twice in `mc:AlternateContent`: DrawingML
/// in `mc:Choice` and VML in `mc:Fallback`. Readers show the first branch that
/// holds text, so text in a later branch is a hidden copy.
fn hidden_copy(node: Node<'_, '_>, format: &str) -> bool {
    node.ancestors().any(|branch| {
        alternate_branch(branch)
            && branch.prev_siblings().skip(1).any(|earlier| {
                // An equation (a14:m) is text a reader shows, though not a:t.
                alternate_branch(earlier)
                    && earlier
                        .descendants()
                        .any(|n| text_node(n, format) || n.has_tag_name((MATH, "t")))
            })
    })
}

/// AlternateContent branches hidden by an earlier branch containing text.
/// Build once for document-wide scans instead of searching earlier subtrees
/// again for every descendant of a fallback branch.
pub(super) struct HiddenBranches {
    branches: BTreeSet<usize>,
}

impl HiddenBranches {
    pub(super) fn new(xml: &Document<'_>, format: &str) -> Self {
        let mut prior_text = BTreeMap::<usize, bool>::new();
        let mut branches = BTreeSet::new();
        for branch in xml.descendants().filter(|n| alternate_branch(*n)) {
            let parent = branch.parent().unwrap();
            let seen = prior_text.entry(parent.range().start).or_default();
            if *seen {
                branches.insert(branch.range().start);
            }
            if !*seen {
                *seen = branch
                    .descendants()
                    .any(|n| text_node(n, format) || n.has_tag_name((MATH, "t")));
            }
        }
        Self { branches }
    }

    pub(super) fn contains(&self, node: Node<'_, '_>) -> bool {
        node.ancestors()
            .any(|ancestor| self.branches.contains(&ancestor.range().start))
    }
}

/// Hidden copies of a visible text element, or of a Word tab or line break,
/// matched by position among its kind within each alternate branch, so that an
/// edit keeps both renderings of a text box equal.
fn hidden_copies<'a, 'input>(
    node: Node<'a, 'input>,
    format: &str,
    cache: &mut BTreeMap<(usize, bool), BranchTexts<'a, 'input>>,
) -> Result<Vec<Node<'a, 'input>>> {
    let text = text_node(node, format);
    let mut copies = vec![];
    for branch in node.ancestors().filter(|n| alternate_branch(*n)) {
        let own = branch_texts(cache, branch, format, text);
        let len = own.nodes.len();
        let index = *own
            .positions
            .get(&node.range().start)
            .context("text element outside its branch")?;
        for other in branch
            .next_siblings()
            .skip(1)
            .filter(|n| alternate_branch(*n))
        {
            let theirs = branch_texts(cache, other, format, text);
            if theirs.nodes.is_empty() {
                continue;
            }
            ensure!(
                theirs.nodes.len() == len,
                "the text box copies in mc:AlternateContent differ; edit this text in Office"
            );
            copies.push(theirs.nodes[index]);
        }
    }
    Ok(copies)
}

struct BranchTexts<'a, 'input> {
    nodes: Vec<Node<'a, 'input>>,
    positions: BTreeMap<usize, usize>,
}

fn branch_texts<'cache, 'a, 'input>(
    cache: &'cache mut BTreeMap<(usize, bool), BranchTexts<'a, 'input>>,
    branch: Node<'a, 'input>,
    format: &str,
    text: bool,
) -> &'cache BranchTexts<'a, 'input> {
    cache
        .entry((branch.range().start, text))
        .or_insert_with(|| {
            let nodes: Vec<_> = branch
                .descendants()
                .filter(|n| {
                    if text {
                        text_node(*n, format)
                    } else if word(format) {
                        word::text_break(*n)
                    } else {
                        n.has_tag_name((DRAWING, "br"))
                    }
                })
                .collect();
            let positions = nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node.range().start, index))
                .collect();
            BranchTexts { nodes, positions }
        })
}

/// `node` written the same way whichever program saved it: attributes sorted
/// by name, no space before `/>`, no whitespace-only text between elements,
/// and text and values with only the characters XML requires escaped. Excel
/// saves the same settings with other spacing and order than other writers.
fn canonical_xml(node: Node<'_, '_>, out: &mut String) {
    let qualified = |namespace: Option<&str>, local: &str| match namespace
        .and_then(|uri| node.lookup_prefix(uri))
        .filter(|prefix| !prefix.is_empty())
    {
        Some(prefix) => format!("{prefix}:{local}"),
        None => local.to_owned(),
    };
    let tag = qualified(node.tag_name().namespace(), node.tag_name().name());
    out.push('<');
    out.push_str(&tag);
    let mut attributes: Vec<_> = node
        .attributes()
        .map(|a| (qualified(a.namespace(), a.name()), a.value()))
        .collect();
    attributes.sort();
    for (name, value) in attributes {
        out.push(' ');
        out.push_str(&name);
        out.push_str("=\"");
        for c in value.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '"' => out.push_str("&quot;"),
                '\t' => out.push_str("&#9;"),
                '\n' => out.push_str("&#10;"),
                '\r' => out.push_str("&#13;"),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    let nested = node.children().any(|c| c.is_element());
    let children: Vec<_> = node
        .children()
        .filter(|c| {
            c.is_element()
                || (c.is_text() && !(nested && c.text().is_some_and(|t| t.trim().is_empty())))
        })
        .collect();
    if children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in children {
        if child.is_element() {
            canonical_xml(child, out);
        } else {
            for c in child.text().unwrap_or_default().chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '\r' => out.push_str("&#13;"),
                    c => out.push(c),
                }
            }
        }
    }
    out.push_str("</");
    out.push_str(&tag);
    out.push('>');
}

pub(crate) fn xml_text(value: &str) -> Result<String> {
    ensure!(value.chars().all(|c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')), "invalid XML character in replacement");
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#13;"))
}

/// Document data that Word content controls show, by store item and XPath.
type BoundData = BTreeMap<(String, String), (word::Binding, String)>;

/// The document data that edited Word content controls are bound to, with the
/// text it takes. Other controls bound to the same data get edits showing
/// that text too, as Word shows it on opening.
fn bound_data_edits<'a, 'input>(
    parts: &[String],
    docs: &'a [Document<'input>],
    edits: &mut [Vec<(Node<'a, 'input>, String)>],
) -> Result<BoundData> {
    let mut data = BoundData::new();
    let mut edited = BTreeSet::new();
    for (index, part_edits) in edits.iter().enumerate() {
        let mut edited_text = BTreeMap::new();
        for (node, value) in part_edits {
            edited_text
                .entry(node.range().start)
                .or_insert(value.as_str());
        }
        let mut controls = vec![];
        let mut control_starts = BTreeSet::new();
        for (node, _) in part_edits {
            if let Some(sdt) = word::bound_control(*node)
                && control_starts.insert(sdt.range().start)
            {
                controls.push(sdt);
            }
        }
        for sdt in controls {
            let place = format!("a content control in {}", parts[index]);
            let binding = word::binding(sdt, &place)?;
            let value = word::control_text(sdt, &edited_text);
            ensure!(
                binding.multiline || !value.contains('\n'),
                "{place} is a single-line text control bound to document data ({}); remove the line break",
                binding.xpath
            );
            let key = (binding.store.clone(), binding.xpath.clone());
            if let Some((_, existing)) = data.get(&key) {
                ensure!(
                    *existing == value,
                    "content controls bound to the same document data ({}) were edited to different text: {existing:?} and {value:?}",
                    binding.xpath
                );
            }
            data.insert(key, (binding, value));
            edited.insert((index, sdt.range().start));
        }
    }
    if data.is_empty() {
        return Ok(data);
    }
    let no_edits = BTreeMap::new();
    for (index, doc) in docs.iter().enumerate() {
        for sdt in doc
            .descendants()
            .filter(|n| n.has_tag_name((WORD, "sdt")) && !hidden_copy(*n, "docx"))
        {
            if edited.contains(&(index, sdt.range().start)) {
                continue;
            }
            let Some((_, value)) = word::binding_key(sdt).and_then(|key| data.get(&key)) else {
                continue;
            };
            if word::control_text(sdt, &no_edits) == *value {
                continue;
            }
            let place = format!("a content control in {}", parts[index]);
            word::binding(sdt, &place)?;
            edits[index].extend(word::fill_control(sdt, value, &place)?);
        }
    }
    Ok(data)
}

/// The part holding data store item `store`: the core or extended document
/// properties, or the custom XML part whose properties name it.
fn bound_data_part(parts: &BTreeMap<String, Vec<u8>>, store: &str) -> Result<String> {
    const CORE_PROPERTIES: &str = "{6C3C8BC8-F283-45AE-878A-BAB7291924A1}";
    const EXTENDED_PROPERTIES: &str = "{6668398D-A668-4E3E-A5EB-62B293D839F1}";
    const CUSTOM_XML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/customXml";
    // The part `base` relates to with a relationship type ending in `kind`.
    let related = |base: &str, kind: &str| -> Result<Option<String>> {
        let (directory, filename) = base.rsplit_once('/').unwrap_or(("", base));
        let rels = if directory.is_empty() {
            format!("_rels/{filename}.rels")
        } else {
            format!("{directory}/_rels/{filename}.rels")
        };
        if !parts.contains_key(&rels) {
            return Ok(None);
        }
        let text = xml_part(parts, &rels)?;
        let doc = Document::parse(&text)?;
        let Some(id) = doc
            .descendants()
            .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
            .find(|n| n.attribute("Type").is_some_and(|t| t.ends_with(kind)))
            .and_then(|n| n.attribute("Id"))
        else {
            return Ok(None);
        };
        relation_target(parts, base, id).map(Some)
    };
    let found = match store {
        CORE_PROPERTIES => related("", "/metadata/core-properties")?,
        EXTENDED_PROPERTIES => related("", "/extended-properties")?,
        _ => {
            let mut found = None;
            for part in parts
                .keys()
                .filter(|p| p.starts_with("customXml/") && !p.contains("/_rels/"))
            {
                let Some(properties) = related(part, "/customXmlProps")? else {
                    continue;
                };
                let text = xml_part(parts, &properties)?;
                let doc = Document::parse(&text)?;
                if doc
                    .root_element()
                    .attribute((CUSTOM_XML, "itemID"))
                    .is_some_and(|id| id.eq_ignore_ascii_case(store))
                {
                    found = Some(part.clone());
                    break;
                }
            }
            found
        }
    };
    found.with_context(|| {
        format!(
            "the document data store {store} a content control shows is missing; edit it in Word"
        )
    })
}

/// The element an XPath of a content control binding selects: a path of
/// `prefix:name[n]` steps from the root, with the prefixes of `prefixes`
/// (`xmlns:ns0='...' xmlns:ns1='...'`).
fn bound_element<'a, 'input>(
    doc: &'a Document<'input>,
    prefixes: &str,
    xpath: &str,
) -> Result<Node<'a, 'input>> {
    static PREFIX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"xmlns:([\w.\-]+)\s*=\s*(?:'([^']*)'|"([^"]*)")"#).unwrap()
    });
    static STEP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(?:([\w.\-]+):)?([\w.\-]+)(?:\[([1-9][0-9]*)\])?$").unwrap()
    });
    let namespaces: BTreeMap<&str, &str> = PREFIX
        .captures_iter(prefixes)
        .map(|c| {
            let uri = c.get(2).or_else(|| c.get(3)).map_or("", |m| m.as_str());
            (c.get(1).map_or("", |m| m.as_str()), uri)
        })
        .collect();
    let unsupported = || {
        format!(
            "the content control binding {xpath} is not a simple element path ARP can follow; edit it in Word"
        )
    };
    let mut node = doc.root();
    for step in xpath
        .strip_prefix('/')
        .with_context(unsupported)?
        .split('/')
    {
        let captures = STEP.captures(step).with_context(unsupported)?;
        let namespace = match captures.get(1) {
            Some(prefix) => Some(*namespaces.get(prefix.as_str()).with_context(unsupported)?),
            None => None,
        };
        let index: usize = captures.get(3).map_or(Ok(1), |n| n.as_str().parse())?;
        node = node
            .children()
            .filter(|c| {
                c.is_element()
                    && c.tag_name().name() == &captures[2]
                    && c.tag_name().namespace() == namespace
            })
            .nth(index - 1)
            .with_context(|| {
                format!(
                    "the document data {xpath} a content control shows is missing; edit it in Word"
                )
            })?;
    }
    ensure!(
        node.is_element() && !node.children().any(|c| c.is_element()),
        "the document data {xpath} a content control shows is not text; edit it in Word"
    );
    Ok(node)
}

/// `text` with each bound element taking its new text.
fn set_bound_values(text: &str, values: &[(&word::Binding, &String)]) -> Result<String> {
    let doc = Document::parse(text)?;
    let mut edits = vec![];
    for (binding, value) in values {
        let element = bound_element(&doc, &binding.prefixes, &binding.xpath)?;
        let raw = &text[element.range()];
        let name = raw[1..]
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .context("invalid bound element")?;
        let (opening, closing) = match raw.strip_suffix("/>") {
            Some(opening) => (format!("{}>", opening.trim_end()), format!("</{name}>")),
            None => (
                raw[..=raw.find('>').context("invalid bound element")?].to_owned(),
                raw[raw.rfind("</").context("invalid bound element")?..].to_owned(),
            ),
        };
        edits.push((
            element.range(),
            format!("{opening}{}{closing}", xml_text(value)?),
        ));
    }
    let result = splice(text, edits, "document data")?;
    Document::parse(&result)?;
    Ok(result)
}

/// `drawing`, an Excel drawing part, with the text of the shapes of `texts`
/// (by `cNvPr` id, the text before and after) changed run by run, as Word text
/// is: the first run a change touches takes it. Line and paragraph breaks and
/// field text stay as they are.
pub fn edit_shape_texts(
    drawing: &str,
    texts: &BTreeMap<String, (String, String)>,
) -> Result<String> {
    const SPREADSHEET_DRAWING: &str =
        "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let doc = Document::parse(drawing)?;
    let mut edits = vec![];
    let mut found = BTreeSet::new();
    for body in doc
        .descendants()
        .filter(|n| n.has_tag_name((SPREADSHEET_DRAWING, "txBody")))
    {
        let Some(id) = body
            .parent_element()
            .into_iter()
            .flat_map(|shape| shape.children())
            .filter(Node::is_element)
            .flat_map(|properties| properties.children())
            .find(|n| n.is_element() && n.tag_name().name() == "cNvPr")
            .and_then(|n| n.attribute("id"))
        else {
            continue;
        };
        let Some((before, after)) = texts.get(id) else {
            continue;
        };
        // Each line (a paragraph or a line break ends one) with its segments.
        let mut lines: Vec<(String, Vec<word::Segment<'_, '_>>)> = vec![(String::new(), vec![])];
        let mut paragraphs = body
            .children()
            .filter(|n| n.has_tag_name((DRAWING, "p")))
            .peekable();
        while let Some(paragraph) = paragraphs.next() {
            for node in paragraph.descendants() {
                if node.has_tag_name((DRAWING, "t")) {
                    // Field text (a:fld) is filled in by Excel.
                    let run = node
                        .parent()
                        .is_some_and(|p| p.has_tag_name((DRAWING, "r")));
                    let (text, segments) = lines.last_mut().context("shape line")?;
                    let start = text.len();
                    text.push_str(node.text().unwrap_or(""));
                    segments.push(word::Segment {
                        node: run.then_some(node),
                        range: start..text.len(),
                    });
                } else if node.has_tag_name((DRAWING, "br")) {
                    lines.push((String::new(), vec![]));
                }
            }
            if paragraphs.peek().is_some() {
                lines.push((String::new(), vec![]));
            }
        }
        // A copy for older readers (mc:Fallback) may hold other text.
        let text: Vec<&str> = lines.iter().map(|(text, _)| text.as_str()).collect();
        if text.join("\n") != *before {
            continue;
        }
        let new_lines: Vec<&str> = after.split('\n').collect();
        ensure!(
            new_lines.len() == lines.len(),
            "the text of shape {id} must keep its {} line(s); edit its lines in Excel",
            lines.len()
        );
        let mut changed = vec![];
        for (number, ((text, segments), new)) in lines.into_iter().zip(new_lines).enumerate() {
            if text != new {
                let block = word::Block::plain(
                    format!("shape {id} line {}", number + 1),
                    text,
                    segments,
                    "Excel",
                );
                changed.extend(word::edit(&block, new)?);
            }
        }
        for (node, new) in changed {
            let raw = &drawing[node.range()];
            let opening =
                raw[..raw.find('>').context("invalid text element")?].trim_end_matches('/');
            let name = opening
                .trim_start_matches('<')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .context("missing text tag")?;
            edits.push((
                node.range(),
                format!("{opening}>{}</{name}>", xml_text(&new)?),
            ));
        }
        found.insert(id.to_owned());
    }
    if let Some(missing) = texts.keys().find(|id| !found.contains(*id)) {
        bail!(
            "shape {missing} with the text it had on import is missing from the drawing; re-import the workbook"
        );
    }
    let result = splice(drawing, edits, "shape text")?;
    Document::parse(&result)?;
    Ok(result)
}

/// `drawing`, an Excel drawing part, with the runs of the shapes of `edits`
/// (by `cNvPr` id) given the edited font. A run without properties gets them;
/// a copy for older readers (`mc:Fallback`) is edited too.
pub fn edit_shape_fonts(
    drawing: &str,
    edits: &BTreeMap<String, crate::fonts::FontEdit>,
) -> Result<String> {
    const SPREADSHEET_DRAWING: &str =
        "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let doc = Document::parse(drawing)?;
    let mut splices = vec![];
    let mut found = BTreeSet::new();
    for body in doc
        .descendants()
        .filter(|n| n.has_tag_name((SPREADSHEET_DRAWING, "txBody")))
    {
        let Some(id) = body
            .parent_element()
            .into_iter()
            .flat_map(|shape| shape.children())
            .filter(Node::is_element)
            .flat_map(|properties| properties.children())
            .find(|n| n.is_element() && n.tag_name().name() == "cNvPr")
            .and_then(|n| n.attribute("id"))
        else {
            continue;
        };
        let Some(edit) = edits.get(id) else {
            continue;
        };
        found.insert(id.to_owned());
        drawing_run_edits(drawing, body, edit, &mut splices)?;
    }
    if let Some(missing) = edits.keys().find(|id| !found.contains(*id)) {
        bail!("shape {missing} has no text in the drawing; re-import the workbook");
    }
    let result = splice(drawing, splices, "shape font")?;
    Document::parse(&result)?;
    Ok(result)
}

/// The edits giving every run of the DrawingML text `body` the font of `edit`,
/// with the paragraph ends that set their own properties.
fn drawing_run_edits(
    source: &str,
    body: Node<'_, '_>,
    edit: &crate::fonts::FontEdit,
    splices: &mut Vec<(std::ops::Range<usize>, String)>,
) -> Result<()> {
    for paragraph in body.children().filter(|n| n.has_tag_name((DRAWING, "p"))) {
        for run in paragraph
            .children()
            .filter(|n| n.has_tag_name((DRAWING, "r")) || n.has_tag_name((DRAWING, "fld")))
        {
            let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, run));
            match run.children().find(|n| n.has_tag_name((DRAWING, "rPr"))) {
                Some(properties) => splices.push((
                    properties.range(),
                    crate::fonts::edited_drawing_run(
                        source,
                        Some(properties),
                        "rPr",
                        prefix,
                        edit,
                    )?,
                )),
                None => {
                    // Run properties come first in a run.
                    let raw = &source[run.range()];
                    let at = run.range().start + raw.find('>').context("invalid run")? + 1;
                    splices.push((
                        at..at,
                        crate::fonts::edited_drawing_run(source, None, "rPr", prefix, edit)?,
                    ));
                }
            }
        }
        if let Some(end) = paragraph
            .children()
            .find(|n| n.has_tag_name((DRAWING, "endParaRPr")))
        {
            let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, end));
            splices.push((
                end.range(),
                crate::fonts::edited_drawing_run(source, Some(end), "endParaRPr", prefix, edit)?,
            ));
        }
    }
    Ok(())
}

/// The namespace prefix of a qualified element name with its colon, such as `w:`.
fn element_prefix(name: &str) -> String {
    name.rsplit_once(':')
        .map_or_else(String::new, |(prefix, _)| format!("{prefix}:"))
}

/// Word run content for `text`: tabs and line breaks become `w:tab` and
/// `w:br`, and the text between them text elements opened with `opening`.
fn word_run_content(text: &str, opening: &str, name: &str, prefix: &str) -> Result<String> {
    let mut content = String::new();
    let mut rest = text;
    loop {
        let end = rest.find(['\n', '\t']).unwrap_or(rest.len());
        if end > 0 {
            content.push_str(&format!("{opening}>{}</{name}>", xml_text(&rest[..end])?));
        }
        let Some(separator) = rest[end..].chars().next() else {
            return Ok(content);
        };
        let element = if separator == '\t' { "tab" } else { "br" };
        content.push_str(&format!("<{prefix}{element}/>"));
        rest = &rest[end + 1..];
    }
}

impl TextSource {
    /// The sheets as JSON with every cell's font and style in place, as the
    /// writers read them: made once, from the typed sheets, when first read.
    fn sheets(&self) -> &[Value] {
        self.json.get_or_init(|| {
            self.extraction
                .iter()
                .map(|sheet| {
                    crate::extraction::sheet_view(sheet)
                        .expect("a sheet read from an original refers to its own tables")
                })
                .collect()
        })
    }

    /// The document with `fonts` (each a `sheet` and `cell` of the original
    /// and the font properties `after` sets) written to the runs of the text,
    /// checked by reading it back; None without font edits.
    fn with_fonts(&self, fonts: &[Value]) -> Result<Option<TextSource>> {
        if fonts.is_empty() {
            return Ok(None);
        }
        let TextBackend::Office(parts) = &self.backend else {
            bail!("font edits are written back to Excel, Word and PowerPoint documents only");
        };
        let mut by_part: BTreeMap<&str, BTreeMap<usize, crate::fonts::FontEdit>> = BTreeMap::new();
        let mut expected: BTreeMap<(&str, &str), crate::fonts::FontEdit> = BTreeMap::new();
        for change in fonts {
            let name = string(&change["sheet"])?;
            let address = string(&change["cell"])?;
            let sheet = self
                .sheets()
                .iter()
                .find(|s| s["name"] == name)
                .with_context(|| format!("font edit page {name} is missing"))?;
            let index = array(&sheet["cells"])?
                .iter()
                .position(|cell| cell["address"] == address)
                .with_context(|| format!("font edit target {name}!{address} is missing"))?;
            let edit = crate::fonts::FontEdit::parse(&change["after"])?;
            if word(&self.format) {
                word_fonts::ensure_word_edit(&edit)?;
            }
            ensure!(
                by_part
                    .entry(string(&sheet["part"])?)
                    .or_default()
                    .insert(index, edit.clone())
                    .is_none(),
                "duplicate font edit of {name}!{address}"
            );
            expected.insert((name, address), edit);
        }
        let mut patched = BTreeMap::new();
        for (part, edits) in by_part {
            let (text, encoding) = part_text(parts.get(part).context("missing document part")?)?;
            let xml = Document::parse(&text)?;
            let drawn = word_drawn_texts(parts, part);
            let blocks = if word(&self.format) {
                word::layout(&xml, &drawn)?.blocks
            } else {
                slide_text::layout(&xml)?.blocks
            };
            let mut cache = BTreeMap::new();
            let mut splices = vec![];
            for (index, edit) in edits {
                let block = blocks.get(index).context("text block disappeared")?;
                splices.extend(if word(&self.format) {
                    word_fonts::run_edits(&text, block, &edit, &self.format, &mut cache)?
                } else {
                    slide_fonts::run_edits(&text, block, &edit, &mut cache)?
                });
            }
            let result = splice(&text, splices, "font")?;
            Document::parse(&result)?;
            patched.insert(part.to_owned(), encode_part(&result, encoding));
        }
        let raw = crate::excel::archive_bytes(&self.raw, &patched)?;
        let Source::Text(written) =
            Source::from_bytes(Path::new(&format!("document.{}", self.format)), raw)?
        else {
            bail!("font edit changed the document format");
        };
        ensure!(
            written.sheets().len() == self.sheets().len(),
            "font edit changed the pages of the document"
        );
        for (old, new) in self.sheets().iter().zip(written.sheets()) {
            let name = string(&old["name"])?;
            let (old_cells, new_cells) = (array(&old["cells"])?, array(&new["cells"])?);
            ensure!(
                old_cells.len() == new_cells.len(),
                "font edit changed the text of {name}"
            );
            for (before, after) in old_cells.iter().zip(new_cells) {
                let address = string(&before["address"])?;
                let wanted = match expected.get(&(name, address)) {
                    Some(edit) => edit.applied(&before["font"]),
                    None => before["font"].clone(),
                };
                ensure!(
                    after["value"] == before["value"]
                        && after["address"] == before["address"]
                        && crate::fonts::same_font(&after["font"], &wanted),
                    "font read-back failed for {name}!{address}: expected {wanted}, found {}",
                    after["font"]
                );
            }
        }
        Ok(Some(written))
    }

    /// Writes the document with `changes` to its text and its `operations`:
    /// for Word, paragraphs and table rows inserted or deleted, and for
    /// PowerPoint, slides copied or deleted. A change names its original cell
    /// (`source_cell`, else `cell`), or for an inserted row its `insertion`,
    /// `offset` and `column`; a change to an inserted slide names the slide.
    #[cfg(test)]
    fn patch(&self, output: &Path, operations: &[Value], changes: &[Value]) -> Result<Value> {
        self.patch_with(output, operations, changes, &BTreeMap::new())
    }

    /// [`TextSource::patch`] with the image `assets` (by asset path) of the
    /// pictures shape operations add.
    fn patch_with(
        &self,
        output: &Path,
        operations: &[Value],
        changes: &[Value],
        assets: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Value> {
        ensure!(
            !matches!(self.backend, TextBackend::Native),
            "text documents use direct editing; edit the original and re-import with the same document ID (export/apply is not supported)"
        );
        ensure!(!output.exists(), "output already exists");
        ensure!(
            operations
                .iter()
                .all(|operation| if is_presentation_operation(operation) {
                    is_powerpoint(&self.format)
                } else {
                    word(&self.format) || is_powerpoint(&self.format) || html(&self.format)
                }),
            "row operations apply to Excel, Word, PowerPoint and HTML documents, slide and shape operations to PowerPoint"
        );
        let slide_operations = parse_slide_operations(operations, self.sheets())?;
        let shape_operations = parse_shape_operations(operations)?;
        let mut shapes_by_sheet: BTreeMap<&str, Vec<&ShapeOperation>> = BTreeMap::new();
        for operation in &shape_operations {
            shapes_by_sheet
                .entry(operation.slide.as_str())
                .or_default()
                .push(operation);
        }
        let restructured = match &self.backend {
            TextBackend::Office(_) if !slide_operations.is_empty() => {
                let complete = office_parts(&self.raw, true)?;
                Some(slides::restructure(
                    &complete,
                    self.sheets(),
                    &slide_operations,
                )?)
            }
            _ => None,
        };
        // The pages as the slide operations leave them, with their parts.
        let view = if slide_operations.is_empty() {
            Cow::Borrowed(self.sheets())
        } else {
            Cow::Owned(slide_view(self.sheets(), &self.layouts, &slide_operations)?)
        };
        let mut pages = vec![];
        for sheet in view.iter() {
            let name = string(&sheet["name"])?;
            let part = match restructured.as_ref().and_then(|r| r.inserted.get(name)) {
                Some(part) => part.clone(),
                None => string(&sheet["part"])?.to_owned(),
            };
            pages.push((name.to_owned(), part));
        }
        let operations = crate::excel::parse_operations(operations, &view)?;
        let mut operations_by_sheet: BTreeMap<&str, Vec<&crate::excel::StructuralOperation>> =
            BTreeMap::new();
        for operation in &operations {
            operations_by_sheet
                .entry(&operation.sheet)
                .or_default()
                .push(operation);
        }
        // Each page and address is looked up for every changed text block.
        let mut sheet_index = BTreeMap::new();
        for sheet in view.iter().chain(self.sheets().iter()) {
            let name = string(&sheet["name"])?;
            if sheet_index.contains_key(name) {
                continue;
            }
            let mut addresses = BTreeMap::new();
            for (index, cell) in array(&sheet["cells"])?.iter().enumerate() {
                addresses.entry(string(&cell["address"])?).or_insert(index);
            }
            sheet_index.insert(name, (sheet, addresses));
        }
        // The replaced texts of each page by name, to check the output against.
        let mut by_page: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
        let mut replacements: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
        let mut inserted: BTreeMap<String, word::InsertedText> = BTreeMap::new();
        for change in changes {
            if change["inserted"] == true {
                let key = (
                    string(&change["insertion"])?.to_owned(),
                    u32::try_from(change["offset"].as_u64().context("inserted offset")?)?,
                    string(&change["column"])?.to_owned(),
                );
                let value = string(&change["after"])
                    .context("text replacement must be a string")?
                    .to_owned();
                inserted
                    .entry(string(&change["sheet"])?.to_owned())
                    .or_default()
                    .insert(key, value);
                continue;
            }
            let name = string(&change["sheet"])?;
            let (sheet, addresses) = sheet_index.get(name).context("missing text container")?;
            let part = match restructured.as_ref().and_then(|r| r.inserted.get(name)) {
                Some(part) => part.as_str(),
                None => string(&sheet["part"])?,
            };
            ensure!(
                !restructured
                    .as_ref()
                    .is_some_and(|r| r.removed.contains(part)),
                "{name} is deleted by a delete_slide operation; its text cannot be edited"
            );
            let address = change.get("source_cell").unwrap_or(&change["cell"]);
            let cell = *addresses
                .get(string(address)?)
                .context("missing text target")?;
            ensure!(
                sheet["cells"][cell]["value"] == change["before"],
                "text baseline mismatch"
            );
            let value = string(&change["after"])
                .context("text replacement must be a string; use an empty string to clear text")?;
            ensure!(
                replacements
                    .entry(part.into())
                    .or_default()
                    .insert(cell, value.into())
                    .is_none(),
                "duplicate text replacement"
            );
            by_page
                .entry(name.to_owned())
                .or_default()
                .insert(cell, value.into());
        }
        match &self.backend {
            TextBackend::Native => unreachable!("native text writeback rejected above"),
            TextBackend::Html(_) if changes.is_empty() && operations.is_empty() => {
                crate::excel::write_unchanged(&self.raw, output)?;
            }
            TextBackend::Html(_) => {
                let no_values = word::InsertedText::new();
                let no_changes = BTreeMap::new();
                let written = html::patch(
                    &self.raw,
                    operations_by_sheet
                        .get("document")
                        .map_or(&[][..], Vec::as_slice),
                    replacements.get("html").unwrap_or(&no_changes),
                    inserted.get("document").unwrap_or(&no_values),
                )?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)?;
                std::io::Write::write_all(&mut file, &written)?;
                file.sync_all()?;
            }
            TextBackend::Office(parts) => {
                ensure!(
                    !parts
                        .keys()
                        .any(|p| p.to_ascii_lowercase().starts_with("_xmlsignatures/")),
                    "signed Office writeback is not supported"
                );
                let (mut patched, removed) = match &restructured {
                    Some(restructured) => {
                        (restructured.patched.clone(), restructured.removed.clone())
                    }
                    None => (BTreeMap::new(), BTreeSet::new()),
                };
                // The image of each picture a shape operation adds, as a media
                // part related from its slide.
                let page_parts: BTreeMap<&str, &str> = pages
                    .iter()
                    .map(|(name, part)| (name.as_str(), part.as_str()))
                    .collect();
                let pictures = self.add_pictures(
                    &shape_operations,
                    &page_parts,
                    restructured.as_ref(),
                    assets,
                    &mut patched,
                )?;
                // Every Word part is read, since a content control bound to
                // edited document data shows it wherever it is; PowerPoint reads
                // the pages whose text or rows change.
                let pages: Vec<(String, String)> = pages
                    .into_iter()
                    .filter(|(name, part)| {
                        word(&self.format)
                            || replacements.contains_key(part)
                            || operations_by_sheet.contains_key(name.as_str())
                            || shapes_by_sheet.contains_key(name.as_str())
                    })
                    .collect();
                let read: Vec<String> = pages.iter().map(|(_, part)| part.clone()).collect();
                let texts = read
                    .iter()
                    .map(|part| {
                        let bytes = match &restructured {
                            Some(restructured) => restructured.part(parts, part),
                            None => parts.get(part).map(Vec::as_slice),
                        };
                        part_text(bytes.context("missing document part")?)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let docs = texts
                    .iter()
                    .map(|(text, _)| Document::parse(text))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut edits: Vec<Vec<(Node<'_, '_>, String)>> = vec![vec![]; read.len()];
                // Text written into empty slide placeholders, by part.
                let mut fills: Vec<Vec<(std::ops::Range<usize>, String)>> =
                    vec![vec![]; read.len()];
                // Paragraphs and table rows inserted or deleted, by part.
                let mut structural = vec![vec![]; read.len()];
                // Shapes changed, added or deleted, by part, and the IDs of the added.
                let mut shaped: Vec<Vec<(std::ops::Range<usize>, String)>> =
                    vec![vec![]; read.len()];
                let mut added_shapes: BTreeMap<String, (String, u64)> = BTreeMap::new();
                let no_values = word::InsertedText::new();
                for (index, part) in read.iter().enumerate() {
                    let sheet_name = pages[index].0.as_str();
                    let part_operations = operations_by_sheet
                        .get(sheet_name)
                        .cloned()
                        .unwrap_or_default();
                    let changes = replacements.get(part);
                    let shape_operations = shapes_by_sheet.get(sheet_name);
                    if changes.is_none() && part_operations.is_empty() && shape_operations.is_none()
                    {
                        continue;
                    }
                    let xml = &docs[index];
                    if word(&self.format) {
                        let fields = word_field_results(xml);
                        let drawn = word_drawn_texts(parts, part);
                        let layout = word::layout(xml, &drawn)?;
                        structural[index] = layout
                            .restructure(
                                &texts[index].0,
                                &part_operations,
                                inserted.get(sheet_name).unwrap_or(&no_values),
                            )
                            .with_context(|| format!("{sheet_name} ({part})"))?;
                        for (i, text) in changes.into_iter().flatten() {
                            let block = layout.blocks.get(*i).context("text block disappeared")?;
                            for (node, new) in word::edit(block, text)? {
                                ensure!(
                                    !fields.contains(&node.range().start),
                                    "Word field result text cannot be edited because Word recalculates it (date, page number, table of contents, cross-reference, etc.): {part} {}; edit the field in Word",
                                    block.address
                                );
                                edits[index].push((node, new));
                            }
                        }
                    } else {
                        let layout = slide_text::layout(xml)?;
                        structural[index] = layout
                            .restructure(
                                &texts[index].0,
                                &part_operations,
                                inserted.get(sheet_name).unwrap_or(&no_values),
                            )
                            .with_context(|| format!("{sheet_name} ({part})"))?;
                        for (i, text) in changes.into_iter().flatten() {
                            let block = layout.blocks.get(*i).context("text block disappeared")?;
                            if let Some(paragraph) = block.empty {
                                if !text.is_empty() {
                                    fills[index].push(slide_text::fill_empty(
                                        &texts[index].0,
                                        paragraph,
                                        text,
                                    )?);
                                }
                                continue;
                            }
                            for (node, new) in word::edit(block, text)? {
                                // Slide numbers and dates are fields PowerPoint fills in again.
                                ensure!(
                                    !node.ancestors().any(|a| a.has_tag_name((DRAWING, "fld"))),
                                    "PowerPoint field text (slide number, date, etc.) cannot be edited because PowerPoint recalculates it: {sheet_name} {}; edit the field in PowerPoint",
                                    block.address
                                );
                                edits[index].push((node, new));
                            }
                        }
                        if let Some(shape_operations) = shape_operations {
                            // A copied slide's shapes keep the IDs of the slide it copies.
                            let origin = view
                                .iter()
                                .find(|sheet| sheet["name"] == sheet_name)
                                .and_then(|sheet| sheet["copy_of"].as_str())
                                .and_then(|origin| {
                                    self.sheets().iter().find(|s| s["name"] == origin)
                                })
                                .and_then(|sheet| sheet["part"].as_str())
                                .unwrap_or(part);
                            let (edits, added) = slide_shapes::slide_edits(
                                &texts[index].0,
                                xml,
                                part,
                                origin,
                                shape_operations,
                                &layout.blocks,
                                pictures.get(part.as_str()).unwrap_or(&BTreeMap::new()),
                            )
                            .with_context(|| sheet_name.to_owned())?;
                            shaped[index] = edits;
                            for (operation, id) in added {
                                added_shapes.insert(operation, (sheet_name.to_owned(), id));
                            }
                        }
                    }
                }
                let bound = if word(&self.format) {
                    bound_data_edits(&read, &docs, &mut edits)?
                } else {
                    BTreeMap::new()
                };
                for ((index, edits), part) in edits.into_iter().enumerate().zip(&read) {
                    if edits.is_empty()
                        && structural[index].is_empty()
                        && fills[index].is_empty()
                        && shaped[index].is_empty()
                    {
                        continue;
                    }
                    let (original, encoding) = &texts[index];
                    let original = original.as_ref();
                    let encoding = *encoding;
                    let part = part.clone();
                    let mut targets = vec![];
                    let mut hidden_cache = BTreeMap::new();
                    for (node, text) in edits {
                        for copy in hidden_copies(node, &self.format, &mut hidden_cache)? {
                            targets.push((copy, text.clone()));
                        }
                        targets.push((node, text));
                    }
                    // Insertions sort ahead of an element removed at the same place.
                    let mut text_edits = std::mem::take(&mut fills[index]);
                    text_edits.extend(std::mem::take(&mut structural[index]));
                    text_edits.extend(std::mem::take(&mut shaped[index]));
                    for (node, text) in targets {
                        if !word(&self.format) {
                            text_edits.push(slide_text::replacement(original, node, &text)?);
                            continue;
                        }
                        let raw = &original[node.range()];
                        let end = raw.find('>').context("invalid text element")?;
                        let opening = &raw[..end];
                        let name = opening
                            .trim_start_matches('<')
                            .split(|c: char| c.is_whitespace() || c == '/')
                            .next()
                            .context("missing text tag")?;
                        if word::text_break(node) {
                            // A tab or line break of the run becomes the new text.
                            let prefix = element_prefix(name);
                            let replacement = word_run_content(
                                &text,
                                &format!("<{prefix}t xml:space=\"preserve\""),
                                &format!("{prefix}t"),
                                &prefix,
                            )?;
                            text_edits.push((node.range(), replacement));
                            continue;
                        }
                        // Preserve attributes, enforcing whitespace preservation for Word.
                        let mut opening = opening.trim_end_matches('/').to_owned();
                        if word(&self.format) {
                            if let Some(attr) = node.attributes().find(|a| {
                                a.namespace() == Some("http://www.w3.org/XML/1998/namespace")
                                    && a.name() == "space"
                            }) {
                                let range = attr.range();
                                opening.replace_range(
                                    range.start - node.range().start
                                        ..range.end - node.range().start,
                                    "xml:space=\"preserve\"",
                                );
                            } else {
                                opening.push_str(" xml:space=\"preserve\"");
                            }
                        }
                        let replacement = if word(&self.format) && text.contains(['\n', '\t']) {
                            word_run_content(&text, &opening, name, &element_prefix(name))?
                        } else {
                            format!("{opening}>{}</{name}>", xml_text(&text)?)
                        };
                        text_edits.push((node.range(), replacement));
                    }
                    let result = splice(original, text_edits, "text")?;
                    Document::parse(&result)?;
                    patched.insert(part, encode_part(&result, encoding));
                }
                // The data bound controls show takes their new text.
                let mut by_part: BTreeMap<String, Vec<(&word::Binding, &String)>> = BTreeMap::new();
                let mut stores: BTreeMap<&str, String> = BTreeMap::new();
                for (binding, value) in bound.values() {
                    let part = match stores.get(binding.store.as_str()) {
                        Some(part) => part.clone(),
                        None => {
                            let part = bound_data_part(parts, &binding.store)?;
                            stores.insert(binding.store.as_str(), part.clone());
                            part
                        }
                    };
                    by_part.entry(part).or_default().push((binding, value));
                }
                for (part, values) in by_part {
                    let (text, encoding) = part_text(&parts[&part])?;
                    let updated = set_bound_values(&text, &values)?;
                    patched.insert(part, encode_part(&updated, encoding));
                }
                if restructured.is_none() {
                    validate_office_binary(&self.raw)?;
                }
                crate::excel::write_archive_without(&self.raw, output, &patched, &removed)?;
                // The text of an added shape follows the slide's other text.
                let mut appended: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for operation in &shape_operations {
                    if let ShapeEdit::Add {
                        text: Some(text), ..
                    } = &operation.edit
                    {
                        appended.entry(operation.slide.clone()).or_default().extend(
                            text.split('\n')
                                .filter(|l| !l.is_empty())
                                .map(str::to_owned),
                        );
                    }
                }
                if let Some(restructured) = &restructured {
                    let rows_changed: BTreeSet<&str> =
                        operations.iter().map(|o| o.sheet.as_str()).collect();
                    self.ensure_slides_read_back(
                        output,
                        restructured,
                        &slide_operations,
                        &by_page,
                        &rows_changed,
                        &appended,
                    )
                    .inspect_err(|_| {
                        let _ = fs::remove_file(output);
                    })?;
                }
                if !shape_operations.is_empty() {
                    let order = match &restructured {
                        Some(restructured) => restructured
                            .order
                            .iter()
                            .map(|(slide, _)| slide.clone())
                            .collect(),
                        None => self
                            .sheets()
                            .iter()
                            .filter_map(|s| s["name"].as_str())
                            .filter(|name| name.starts_with("slide-"))
                            .map(str::to_owned)
                            .collect::<Vec<_>>(),
                    };
                    slide_shapes::ensure_read_back(
                        output,
                        &order,
                        &shape_operations,
                        &added_shapes,
                        assets,
                    )
                    .inspect_err(|_| {
                        let _ = fs::remove_file(output);
                    })?;
                }
            }
            TextBackend::Pdf {
                doc: original,
                unparsed,
                ..
            } => {
                // lopdf decrypts a PDF that opens without a password (one with only
                // an editing or printing restriction) and would save it unprotected.
                ensure!(
                    !original.was_encrypted(),
                    "encrypted PDF writeback is not supported, including PDFs protected only against editing or printing"
                );
                ensure!(
                    !original
                        .objects
                        .values()
                        .any(|o| o.as_dict().is_ok_and(|d| d.get(b"ByteRange").is_ok()
                            || d.get(b"Type")
                                .and_then(Object::as_name)
                                .is_ok_and(|v| v == b"Sig"))),
                    "signed PDF writeback is not supported"
                );
                // Saving through lopdf re-serializes every object even without edits.
                if replacements.is_empty() {
                    crate::excel::write_unchanged(&self.raw, output)?;
                } else {
                    let mut doc = original.clone();
                    let glyphs = subset_glyphs(original)?;
                    let mapped = pdf_mapped_fonts(original);
                    let mut read = PdfFonts::new(&mapped);
                    let tags = pdf_tags::read(original);
                    for (page, id) in original.get_pages() {
                        if let Some(changes) = replacements.get(&format!("pdf:{page}")) {
                            ensure!(
                                !unparsed.contains(&page),
                                "PDF page {page} has drawing commands ARP cannot read to the end, so it cannot be rewritten"
                            );
                            let raw = original.get_page_content_with_limit(id, MAX_PAGE)?;
                            let mut content = Content::decode_strict(&raw)?;
                            // The strings each changed cell is made of take
                            // what the change gives them.
                            let strings = pdf_text(
                                original,
                                id,
                                &mut content,
                                &BTreeMap::new(),
                                None,
                                &mut read,
                            )?;
                            let (layout, ..) =
                                pdf_page_layout(original, id, &content, &strings, &tags);
                            let mut rewritten = BTreeMap::new();
                            for (cell, after) in changes {
                                let cell =
                                    layout.cells.get(*cell).context("PDF text target missing")?;
                                rewritten.extend(page_layout::distribute(
                                    &cell.pieces,
                                    |i| strings[i].text.as_str(),
                                    after,
                                )?);
                            }
                            pdf_text(
                                original,
                                id,
                                &mut content,
                                &rewritten,
                                Some(&glyphs),
                                &mut read,
                            )?;
                            // Always allocate a new stream: an original stream may be shared by pages.
                            let stream = doc.add_object(Stream::new(
                                lopdf::Dictionary::new(),
                                encode_content(&raw, &content)
                                    .with_context(|| format!("PDF page {page}"))?,
                            ));
                            doc.get_object_mut(id)?
                                .as_dict_mut()?
                                .set("Contents", Object::Reference(stream));
                        }
                    }
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(output)?;
                    doc.save_to(&mut file)?;
                    file.sync_all()?;
                }
            }
        }
        let mut report = json!({"format":self.format,"text_changes":changes.len(),"layout_review_required":true});
        if let Some(restructured) = &restructured {
            report["slide_order"] = json!(
                restructured
                    .order
                    .iter()
                    .map(|(slide, _)| slide)
                    .collect::<Vec<_>>()
            );
        }
        Ok(report)
    }

    /// The relationship ID of each picture asset in the slide parts of the
    /// pictures `operations` add or replace: a new media part related from the
    /// slide, written into `patched`.
    fn add_pictures(
        &self,
        operations: &[ShapeOperation],
        pages: &BTreeMap<&str, &str>,
        restructured: Option<&slides::Restructured>,
        assets: &BTreeMap<String, Vec<u8>>,
        patched: &mut BTreeMap<String, Vec<u8>>,
    ) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
        let mut pictures: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        if !operations.iter().any(|o| o.asset().is_some()) {
            return Ok(pictures);
        }
        let TextBackend::Office(parts) = &self.backend else {
            bail!("pictures are added to PowerPoint slides only");
        };
        let complete = office_parts(&self.raw, true)?;
        let mut taken: BTreeSet<String> = complete
            .keys()
            .chain(patched.keys())
            .map(|name| name.to_lowercase())
            .collect();
        let mut media: BTreeMap<String, String> = BTreeMap::new();
        for operation in operations {
            let Some(asset) = operation.asset() else {
                continue;
            };
            let bytes = assets
                .get(asset)
                .with_context(|| format!("image asset missing: {asset}"))?;
            let slide = *pages
                .get(operation.slide.as_str())
                .with_context(|| format!("{} is not a slide here", operation.slide))?;
            let part = match media.get(asset) {
                Some(part) => part.clone(),
                None => {
                    let part = (1..)
                        .map(|n| format!("ppt/media/arp-image{n}.png"))
                        .find(|name| !taken.contains(&name.to_lowercase()))
                        .unwrap();
                    taken.insert(part.to_lowercase());
                    patched.insert(part.clone(), bytes.clone());
                    media.insert(asset.to_owned(), part.clone());
                    part
                }
            };
            if pictures.get(slide).is_some_and(|p| p.contains_key(asset)) {
                continue;
            }
            let (directory, file) = slide
                .rsplit_once('/')
                .context("slide part without folder")?;
            let rels = format!("{directory}/_rels/{file}.rels");
            let text = match patched.get(&rels) {
                Some(bytes) => part_text(bytes)?.0.into_owned(),
                None => match restructured
                    .and_then(|r| r.part(parts, &rels))
                    .or_else(|| parts.get(&rels).map(Vec::as_slice))
                {
                    Some(bytes) => part_text(bytes)?.0.into_owned(),
                    None => format!(r#"<Relationships xmlns="{PACKAGE_REL}"></Relationships>"#),
                },
            };
            let xml = Document::parse(&text)?;
            let used: BTreeSet<&str> = xml
                .descendants()
                .filter_map(|n| n.attribute("Id"))
                .collect();
            let id = (1..)
                .map(|n| format!("rIdArp{n}"))
                .find(|id| !used.contains(id.as_str()))
                .unwrap();
            let target = format!("../media/{}", part.rsplit('/').next().unwrap_or(&part));
            let end = text
                .rfind("</")
                .context("relationships part without closing tag")?;
            let mut updated = text.clone();
            updated.insert_str(
                end,
                &format!(
                    r#"<Relationship Id="{id}" Type="{}" Target="{target}"/>"#,
                    slide_shapes::IMAGE
                ),
            );
            patched.insert(rels, updated.into_bytes());
            pictures
                .entry(slide.to_owned())
                .or_default()
                .insert(asset.to_owned(), id);
        }
        // PNG parts need their content type.
        let types = "[Content_Types].xml";
        let text = match patched.get(types).or_else(|| parts.get(types)) {
            Some(bytes) => part_text(bytes)?.0.into_owned(),
            None => bail!("the presentation has no content types"),
        };
        let xml = Document::parse(&text)?;
        if !xml.descendants().any(|n| {
            n.tag_name().name() == "Default"
                && n.attribute("Extension")
                    .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        }) {
            let end = text
                .rfind("</")
                .context("content types without closing tag")?;
            let mut updated = text.clone();
            updated.insert_str(end, r#"<Default Extension="png" ContentType="image/png"/>"#);
            patched.insert(types.to_owned(), updated.into_bytes());
        }
        Ok(pictures)
    }

    /// Reads the written presentation back and compares its slides and notes
    /// pages, in order, with the pages the operations and `edits` should make.
    /// A page whose rows change (`rows_changed`) is compared by its place only.
    fn ensure_slides_read_back(
        &self,
        output: &Path,
        restructured: &slides::Restructured,
        operations: &[SlideOperation],
        edits: &BTreeMap<String, BTreeMap<usize, String>>,
        rows_changed: &BTreeSet<&str>,
        appended: &BTreeMap<String, Vec<String>>,
    ) -> Result<()> {
        // Every page as the operations leave it: kept, copied or made from a layout.
        let view = slide_view(self.sheets(), &self.layouts, operations)?;
        let sheets: BTreeMap<&str, &Value> = view
            .iter()
            .map(|sheet| Ok((string(&sheet["name"])?, sheet)))
            .collect::<Result<_>>()?;
        let expected_page = |name: &str| -> Result<Option<Vec<String>>> {
            if rows_changed.contains(name) {
                return Ok(None);
            }
            let sheet = sheets.get(name).context("missing text container")?;
            let mut values = array(&sheet["cells"])?
                .iter()
                .map(|cell| cell["value"].as_str().unwrap_or("").to_owned())
                .collect::<Vec<_>>();
            for (index, text) in edits.get(name).into_iter().flatten() {
                values[*index] = text.clone();
            }
            values.extend(appended.get(name).into_iter().flatten().cloned());
            Ok(Some(values))
        };
        let mut expected = vec![];
        for (slide, _) in &restructured.order {
            expected.push(expected_page(slide)?);
        }
        for (_, notes) in &restructured.order {
            if let Some(notes) = notes {
                expected.push(expected_page(notes)?);
            }
        }
        let Source::Text(written) = Source::open(output)? else {
            bail!("the written presentation is not read as a presentation");
        };
        let actual = written
            .sheets()
            .iter()
            .map(|sheet| {
                Ok(array(&sheet["cells"])?
                    .iter()
                    .map(|cell| cell["value"].as_str().unwrap_or("").to_owned())
                    .collect::<Vec<_>>())
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(&expected)
                    .all(|(actual, expected)| expected.as_ref().is_none_or(|e| e == actual)),
            "the written presentation does not read back as the slide operations and edits make it; nothing is written"
        );
        for ((slide, _), (sheet, hidden)) in restructured
            .order
            .iter()
            .zip(written.sheets().iter().zip(&restructured.hidden))
        {
            ensure!(
                (sheet["state"] == "hidden") == *hidden,
                "{slide} does not read back {} in the slide show; nothing is written",
                if *hidden { "hidden" } else { "shown" }
            );
        }
        Ok(())
    }
}

/// The byte ranges of the inline images (`BI` ... `ID` data `EI`) of a page's
/// content stream, outside strings and comments. `lengths` gives, in order,
/// the data length of each image lopdf could read, which marks its end
/// exactly; otherwise the image ends at the first `EI` set off by whitespace,
/// as PDF readers find it.
fn inline_images(raw: &[u8], lengths: &[Option<usize>]) -> Result<Vec<std::ops::Range<usize>>> {
    let space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0');
    let delimiter = |b: u8| space(b) || b"()<>[]{}/%".contains(&b);
    let token = |at: usize, word: &[u8]| {
        raw[at..].starts_with(word)
            && (at == 0 || delimiter(raw[at - 1]))
            && raw.get(at + word.len()).is_none_or(|b| delimiter(*b))
    };
    // The end of the name, string, comment or hex string starting at `at`.
    let skip = |at: usize| -> Option<usize> {
        match raw[at] {
            b'/' => Some(
                raw[at + 1..]
                    .iter()
                    .position(|b| delimiter(*b))
                    .map_or(raw.len(), |p| at + 1 + p),
            ),
            b'%' => Some(
                raw[at..]
                    .iter()
                    .position(|b| matches!(b, b'\r' | b'\n'))
                    .map_or(raw.len(), |p| at + p),
            ),
            b'(' => {
                let (mut depth, mut i) = (0usize, at);
                while i < raw.len() {
                    match raw[i] {
                        b'\\' => i += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(i + 1);
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                Some(raw.len())
            }
            b'<' if raw.get(at + 1) != Some(&b'<') => Some(
                raw[at..]
                    .iter()
                    .position(|b| *b == b'>')
                    .map_or(raw.len(), |p| at + p + 1),
            ),
            _ => None,
        }
    };
    let mut images = vec![];
    let mut i = 0;
    while i < raw.len() {
        if let Some(end) = skip(i) {
            i = end;
            continue;
        }
        if !token(i, b"BI") {
            i += 1;
            continue;
        }
        let start = i;
        i += 2;
        while i < raw.len() && !token(i, b"ID") {
            i = skip(i).unwrap_or(i + 1);
        }
        ensure!(i < raw.len(), "a PDF inline image has no ID");
        let mut data = i + 2;
        let exact = lengths
            .get(images.len())
            .copied()
            .flatten()
            .or_else(|| inline_length(&raw[start + 2..i]))
            .and_then(|length| {
                while data < raw.len() && space(raw[data]) {
                    data += 1;
                }
                let mut end = data.checked_add(length)?;
                while end < raw.len() && space(raw[end]) {
                    end += 1;
                }
                token(end, b"EI").then_some(end + 2)
            });
        let end = match exact {
            Some(end) => end,
            None => (i + 3..raw.len().saturating_sub(1))
                .find(|&at| space(raw[at - 1]) && token(at, b"EI"))
                .map(|at| at + 2)
                .context("a PDF inline image has no EI")?,
        };
        images.push(start..end);
        i = end;
    }
    Ok(images)
}

/// The protection that keeps the original `raw` of `format` from being read:
/// the encryption of an Office package, or of a PDF that needs a password or
/// wraps an encrypted document. A PDF restricted only against editing or
/// printing opens without a password and is not protected so.
pub fn protection(format: &str, raw: &[u8]) -> Option<encryption::Protected> {
    if format == "pdf" {
        pdf_protection(&pdf_load(raw).ok()?.0)
    } else if is_office(format) {
        encryption::protection(raw).map(encryption::Protected::office)
    } else {
        None
    }
}

/// Whether `format` is an Open XML package of Excel, Word or PowerPoint.
pub fn is_office(format: &str) -> bool {
    is_excel(format) || WORD_FORMATS.contains(&format) || is_powerpoint(format)
}

/// The protection of the loaded PDF `doc` that keeps its pages from being read.
fn pdf_protection(doc: &Pdf) -> Option<encryption::Protected> {
    if doc.is_encrypted() || doc.trailer.get(b"Encrypt").is_ok() {
        return Some(encryption::Protected::pdf_password());
    }
    pdf_encrypted_payload(doc).map(|payload| encryption::Protected::pdf_payload(&payload))
}

/// The protection of the encrypted document `doc` wraps, when `doc` is an
/// unencrypted wrapper (ISO 32000-2, 7.6.7): a cover page shown by readers
/// that cannot open the document, which is embedded as a file whose
/// relationship is EncryptedPayload. Sensitivity labels and IRM protect PDFs
/// so (`MicrosoftIRMServices`). The name is the payload's cryptographic
/// filter, or `encrypted payload` without one.
fn pdf_encrypted_payload(doc: &Pdf) -> Option<String> {
    doc.objects.values().find_map(|object| {
        let specification = match object {
            Object::Dictionary(dictionary) => dictionary,
            Object::Stream(stream) => &stream.dict,
            _ => return None,
        };
        let payload = specification
            .get(b"AFRelationship")
            .and_then(Object::as_name)
            .is_ok_and(|name| name == b"EncryptedPayload");
        payload.then(|| {
            specification
                .get(b"EP")
                .and_then(|ep| doc.dereference(ep))
                .and_then(|(_, ep)| ep.as_dict())
                .and_then(|ep| ep.get(b"Subtype"))
                .and_then(Object::as_name)
                .map_or_else(
                    |_| "encrypted payload".to_owned(),
                    |name| String::from_utf8_lossy(name).into_owned(),
                )
        })
    })
}

/// The PDF `raw` holds, and whether it had to be repaired. A file whose
/// cross-reference table does not lead to its pages (offsets moved by bytes
/// before its header or by an edit, a table that is missing, an encryption
/// dictionary written in the trailer, which lopdf does not take) is read as
/// PDF readers read it: its objects found by reading it through, and its
/// trailer's entries kept.
fn pdf_load(raw: &[u8]) -> Result<(Pdf, bool)> {
    let loaded = Pdf::load_mem(raw);
    if loaded.as_ref().is_ok_and(|doc| !doc.get_pages().is_empty()) {
        return Ok((loaded?, false));
    }
    if let Some(rebuilt) = pdf_rebuilt(raw)
        && let Ok(doc) = Pdf::load_mem(&rebuilt)
        && !doc.get_pages().is_empty()
    {
        return Ok((doc, true));
    }
    Ok((loaded?, false))
}

/// `raw` from its header on, with a cross-reference table of the objects
/// found in it (the last of each number) and a trailer naming the root,
/// information, ID and encryption of its last trailer (or cross-reference
/// stream), or the catalog found; none when it holds no catalog.
fn pdf_rebuilt(raw: &[u8]) -> Option<Vec<u8>> {
    let start = raw.windows(5).position(|w| w == b"%PDF-")?;
    let mut out = raw[start..].to_vec();
    let body = out.clone();
    let space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0');
    let digits_before = |end: usize| -> Option<(usize, u64)> {
        let from = body[..end]
            .iter()
            .rposition(|b| !b.is_ascii_digit())
            .map_or(0, |p| p + 1);
        let number = std::str::from_utf8(&body[from..end]).ok()?.parse().ok()?;
        (from < end).then_some((from, number))
    };
    // "N G obj" at the start of a line or after whitespace.
    let mut objects: BTreeMap<u64, (u64, usize)> = BTreeMap::new();
    let mut at = 0;
    while let Some(found) = body[at..].windows(3).position(|w| w == b"obj") {
        let obj = at + found;
        at = obj + 3;
        if body
            .get(obj + 3)
            .is_some_and(|b| !space(*b) && !b"<[/(%".contains(b))
        {
            continue;
        }
        let mut end = obj;
        while end > 0 && space(body[end - 1]) {
            end -= 1;
        }
        let Some((gen_start, generation)) = digits_before(end) else {
            continue;
        };
        let mut end = gen_start;
        while end > 0 && space(body[end - 1]) {
            end -= 1;
        }
        if end == gen_start {
            continue;
        }
        let Some((number_start, number)) = digits_before(end) else {
            continue;
        };
        if number_start > 0 && !space(body[number_start - 1]) {
            continue;
        }
        objects.insert(number, (generation, number_start));
    }
    // The entries of the last trailer, else of the last cross-reference
    // stream, else the catalog.
    let dictionary_at = |from: usize| -> Option<&[u8]> {
        let open = from + body[from..].windows(2).position(|w| w == b"<<")?;
        let end = pdf_value_end(&body[open..])?;
        Some(&body[open..open + end])
    };
    let mut trailer = body
        .windows(7)
        .rposition(|w| w == b"trailer")
        .and_then(dictionary_at);
    if trailer.is_none_or(|t| entry(t, b"/Root").is_none()) {
        // The cross-reference stream written last in the file.
        trailer = objects
            .values()
            .filter_map(|(_, offset)| Some((*offset, dictionary_at(*offset)?)))
            .filter(|(_, d)| {
                let d = String::from_utf8_lossy(d).replace(' ', "");
                d.contains("/Type/XRef") && d.contains("/Root")
            })
            .max_by_key(|(offset, _)| *offset)
            .map(|(_, d)| d);
    }
    let root = match trailer.and_then(|t| entry(t, b"/Root")) {
        Some(root) => root.to_vec(),
        None => {
            let (number, (generation, _)) = objects.iter().find(|(_, (_, offset))| {
                dictionary_at(*offset).is_some_and(|d| {
                    String::from_utf8_lossy(d)
                        .replace(' ', "")
                        .contains("/Type/Catalog")
                })
            })?;
            format!("{number} {generation} R").into_bytes()
        }
    };
    let mut entries = b"/Root ".to_vec();
    entries.extend_from_slice(&root);
    for key in [b"/Info".as_slice(), b"/ID"] {
        if let Some(value) = trailer.and_then(|t| entry(t, key)) {
            entries.extend_from_slice(b" ");
            entries.extend_from_slice(key);
            entries.extend_from_slice(b" ");
            entries.extend_from_slice(value);
        }
    }
    let mut size = objects.keys().next_back().map_or(1, |n| n + 1);
    if let Some(encrypt) = trailer.and_then(|t| entry(t, b"/Encrypt")) {
        // A dictionary written in the trailer is made an object of its own.
        let reference = if encrypt.starts_with(b"<<") {
            let number = size;
            size += 1;
            out.extend_from_slice(b"\n");
            objects.insert(number, (0, out.len()));
            out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
            out.extend_from_slice(encrypt);
            out.extend_from_slice(b"\nendobj\n");
            format!("{number} 0 R").into_bytes()
        } else {
            encrypt.to_vec()
        };
        entries.extend_from_slice(b" /Encrypt ");
        entries.extend_from_slice(&reference);
    }
    out.extend_from_slice(b"\n");
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f\r\n").as_bytes());
    for number in 1..size {
        match objects.get(&number) {
            Some((generation, offset)) => {
                out.extend_from_slice(format!("{offset:010} {generation:05} n\r\n").as_bytes())
            }
            None => out.extend_from_slice(b"0000000000 65535 f\r\n"),
        }
    }
    out.extend_from_slice(format!("trailer\n<< /Size {size} ").as_bytes());
    out.extend_from_slice(&entries);
    out.extend_from_slice(format!(" >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    Some(out)
}

/// The value of `key` in the dictionary `dict` as written: a reference
/// (`N G R`), a dictionary or an array.
fn entry<'a>(dict: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let at = dict
        .windows(key.len())
        .position(|w| w == key)
        .filter(|at| {
            dict.get(at + key.len())
                .is_some_and(|b| !b.is_ascii_alphanumeric())
        })?
        + key.len();
    let rest = &dict[at..];
    let start = rest.iter().position(|b| !b.is_ascii_whitespace())?;
    let rest = &rest[start..];
    match rest.first()? {
        b'0'..=b'9' => {
            let end = rest.iter().position(|b| *b == b'R')?;
            Some(&rest[..=end])
        }
        b'<' | b'[' => Some(&rest[..pdf_value_end(rest)?]),
        _ => None,
    }
}

/// Where the dictionary or array starting `text` ends, past its strings
/// (a hex string's `>` before a dictionary's `>>` ends the string).
pub(super) fn pdf_value_end(text: &[u8]) -> Option<usize> {
    let (mut depth, mut i) = (0usize, 0);
    while i < text.len() {
        match text[i] {
            b'<' if text.get(i + 1) == Some(&b'<') => {
                depth += 1;
                i += 2;
                continue;
            }
            b'<' => i += text[i..].iter().position(|b| *b == b'>')?,
            b'(' => {
                let mut nested = 0usize;
                while i < text.len() {
                    match text[i] {
                        b'\\' => i += 1,
                        b'(' => nested += 1,
                        b')' => {
                            nested -= 1;
                            if nested == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'[' => depth += 1,
            b'>' if text.get(i + 1) == Some(&b'>') => {
                depth = depth.checked_sub(1)?;
                i += 2;
                if depth == 0 {
                    return Some(i);
                }
                continue;
            }
            b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The length of the data of an inline image without a filter, from the
/// entries `dictionary` (between BI and ID) gives: lopdf leaves it out when
/// the image names its color space abbreviated (`/G`) or in the resources,
/// and the data can hold bytes that look like its EI. None for a filtered
/// image, or one whose color space it does not tell.
pub(super) fn inline_length(dictionary: &[u8]) -> Option<usize> {
    let mut written = b"<<".to_vec();
    written.extend_from_slice(dictionary);
    written.extend_from_slice(b">> BI");
    let operations = Content::decode(&written).ok()?.operations;
    let dict = operations.first()?.operands.first()?.as_dict().ok()?;
    let entry = |short: &[u8], long: &[u8]| dict.get(short).or_else(|_| dict.get(long)).ok();
    if entry(b"F", b"Filter").is_some() {
        return None;
    }
    let number = |short: &[u8], long: &[u8]| {
        entry(short, long)?
            .as_i64()
            .ok()
            .and_then(|n| usize::try_from(n).ok())
    };
    let (width, height) = (number(b"W", b"Width")?, number(b"H", b"Height")?);
    let mask = entry(b"IM", b"ImageMask").is_some_and(|m| matches!(m.as_bool(), Ok(true)));
    let (components, bits) = if mask {
        (1, 1)
    } else {
        let components = match entry(b"CS", b"ColorSpace")? {
            Object::Name(name) => match name.as_slice() {
                b"G" | b"DeviceGray" | b"CalGray" => 1,
                b"RGB" | b"DeviceRGB" | b"CalRGB" => 3,
                b"CMYK" | b"DeviceCMYK" => 4,
                _ => return None,
            },
            // An indexed space written out: one index a sample.
            Object::Array(space)
                if space
                    .first()
                    .is_some_and(|f| matches!(f.as_name(), Ok(b"I" | b"Indexed"))) =>
            {
                1
            }
            _ => return None,
        };
        (components, number(b"BPC", b"BitsPerComponent")?)
    };
    width
        .checked_mul(components)?
        .checked_mul(bits)?
        .div_ceil(8)
        .checked_mul(height)
}

/// The operations of the content stream `raw`, and whether all of it was
/// read: strictly, else as far as lopdf's lenient reader goes. lopdf finds
/// the end of an inline image it cannot read by the whitespace after its EI,
/// which a stream ending with the image lacks, so a newline is read after a
/// stream that does not end with whitespace.
fn pdf_content(raw: &[u8]) -> Result<(Content, bool)> {
    let padded;
    let raw = if raw.last().is_some_and(|b| !b.is_ascii_whitespace()) {
        padded = [raw, b"\n"].concat();
        &padded[..]
    } else {
        raw
    };
    Ok(match Content::decode_strict(raw) {
        Ok(content) => (content, true),
        Err(_) => (Content::decode(raw)?, false),
    })
}

/// The resource dictionaries page `id` uses: its own, then each of the page
/// tree nodes above it, nearest first, as a page inherits Resources. lopdf's
/// own lookup leaves out an inherited dictionary written in its node.
fn pdf_page_resources(doc: &Pdf, id: lopdf::ObjectId) -> Vec<&lopdf::Dictionary> {
    let mut found = vec![];
    let mut seen = BTreeSet::from([id]);
    let mut node = doc.get_dictionary(id).ok();
    while let Some(dict) = node {
        if let Some(resources) = dict.get(b"Resources").ok().and_then(|r| match r {
            Object::Reference(id) => doc.get_dictionary(*id).ok(),
            r => r.as_dict().ok(),
        }) {
            found.push(resources);
        }
        node = dict
            .get(b"Parent")
            .and_then(Object::as_reference)
            .ok()
            .filter(|parent| seen.insert(*parent))
            .and_then(|parent| doc.get_dictionary(parent).ok());
    }
    found
}

/// The fonts page `id` names, by name, from the nearest resources naming each.
fn pdf_page_fonts(doc: &Pdf, id: lopdf::ObjectId) -> BTreeMap<Vec<u8>, &lopdf::Dictionary> {
    pdf_resource_fonts(doc, &pdf_page_resources(doc, id))
}

/// The fonts `resources` name, by name, from the first naming each.
fn pdf_resource_fonts<'a>(
    doc: &'a Pdf,
    resources: &[&'a lopdf::Dictionary],
) -> BTreeMap<Vec<u8>, &'a lopdf::Dictionary> {
    let mut fonts = BTreeMap::new();
    for resources in resources {
        let Some(named) = resources.get(b"Font").ok().and_then(|f| match f {
            Object::Reference(id) => doc.get_dictionary(*id).ok(),
            f => f.as_dict().ok(),
        }) else {
            continue;
        };
        for (name, font) in named.iter() {
            let font = match font {
                Object::Reference(id) => doc.get_dictionary(*id).ok(),
                font => font.as_dict().ok(),
            };
            if let Some(font) = font {
                fonts.entry(name.clone()).or_insert(font);
            }
        }
    }
    fonts
}

/// Encodes `content` decoded from `raw`, copying its inline images from `raw`
/// as they were: lopdf would write one as a stream object, and drops those it
/// cannot read.
fn encode_content(raw: &[u8], content: &Content) -> Result<Vec<u8>> {
    let lengths: Vec<_> = content
        .operations
        .iter()
        .filter(|o| o.operator == "BI")
        .map(|o| match o.operands.first() {
            Some(Object::Stream(stream)) => Some(stream.content.len()),
            _ => None,
        })
        .collect();
    let images = inline_images(raw, &lengths)?;
    ensure!(
        images.len() == lengths.len(),
        "the inline images of a PDF page could not be told apart from its other drawing commands; edit the text in a PDF editor"
    );
    let mut output = vec![];
    let mut pending = vec![];
    let mut images = images.into_iter();
    let flush =
        |output: &mut Vec<u8>, pending: &mut Vec<lopdf::content::Operation>| -> Result<()> {
            if !pending.is_empty() {
                output.extend(
                    Content {
                        operations: std::mem::take(pending),
                    }
                    .encode()?,
                );
                output.push(b'\n');
            }
            Ok(())
        };
    for operation in &content.operations {
        if operation.operator == "BI" {
            flush(&mut output, &mut pending)?;
            output.extend_from_slice(&raw[images.next().context("inline image missing")?]);
            output.push(b'\n');
        } else {
            pending.push(operation.clone());
        }
    }
    flush(&mut output, &mut pending)?;
    Ok(output)
}

enum PdfEncoding<'a> {
    /// `width` is the length of a character code in bytes.
    Mapped {
        encoding: lopdf::Encoding<'a>,
        width: usize,
    },
    Unicode {
        ucs2: bool,
    },
    /// A composite font whose codes say no text ARP can read: neither a
    /// Unicode CMap nor a ToUnicode map gives their characters. Its strings
    /// are left out of the page's text and kept as they are.
    Unreadable,
}

/// Whether the predefined CMap `name` has Unicode character codes, big-endian
/// without a BOM: `Some(true)` for UCS-2, `Some(false)` for UTF-16. The
/// half-width (`HW`) and proportional (`Pro`) forms map the same codes to
/// other glyphs. Identity-H/V are CID codes, not Unicode; they need a
/// ToUnicode map.
fn unicode_cmap(name: &[u8]) -> Option<bool> {
    match name {
        b"UniJIS-UCS2-H"
        | b"UniJIS-UCS2-V"
        | b"UniJIS-UCS2-HW-H"
        | b"UniJIS-UCS2-HW-V"
        | b"UniJISPro-UCS2-V"
        | b"UniJISPro-UCS2-HW-V"
        | b"UniGB-UCS2-H"
        | b"UniGB-UCS2-V"
        | b"UniCNS-UCS2-H"
        | b"UniCNS-UCS2-V"
        | b"UniKS-UCS2-H"
        | b"UniKS-UCS2-V" => Some(true),
        b"UniJIS-UTF16-H"
        | b"UniJIS-UTF16-V"
        | b"UniJIS2004-UTF16-H"
        | b"UniJIS2004-UTF16-V"
        | b"UniGB-UTF16-H"
        | b"UniGB-UTF16-V"
        | b"UniCNS-UTF16-H"
        | b"UniCNS-UTF16-V"
        | b"UniKS-UTF16-H"
        | b"UniKS-UTF16-V"
        | b"UniAKR-UTF16-H" => Some(false),
        _ => None,
    }
}

/// Whether every code space of the ToUnicode map of `font` has two-byte
/// codes, as ARP reads the codes of a composite font.
fn two_byte_to_unicode(font: &lopdf::Dictionary, doc: &Pdf) -> bool {
    let Ok(stream) = font
        .get_deref(b"ToUnicode", doc)
        .and_then(Object::as_stream)
    else {
        return false;
    };
    let Ok(map) = stream.get_plain_content_with_limit(MAX_PAGE) else {
        return false;
    };
    let text = String::from_utf8_lossy(&map);
    let mut spaces = text
        .split("begincodespacerange")
        .skip(1)
        .filter_map(|block| block.split("endcodespacerange").next())
        .flat_map(|block| block.split('<').skip(1))
        .map(|code| code.split('>').next().unwrap_or_default().trim().len())
        .peekable();
    spaces.peek().is_some() && spaces.all(|digits| digits == 4)
}

/// The font `resource` of a page names, as the note on text ARP leaves out
/// gives it: its BaseFont, its Encoding (a CMap name, or the name of an
/// embedded CMap), whether it has a ToUnicode map, and its object number.
fn unreadable_font(doc: &Pdf, resource: &[u8], font: &lopdf::Dictionary) -> String {
    let text = |name: &[u8]| String::from_utf8_lossy(name).into_owned();
    let base = font
        .get(b"BaseFont")
        .and_then(Object::as_name)
        .map_or_else(|_| "なし".to_owned(), text);
    let encoding = match font.get_deref(b"Encoding", doc) {
        Ok(Object::Name(name)) => text(name),
        Ok(Object::Stream(cmap)) => match cmap.dict.get(b"CMapName").and_then(Object::as_name) {
            Ok(name) => format!("埋め込みCMap {}", text(name)),
            Err(_) => "埋め込みCMap".to_owned(),
        },
        _ => "なし".to_owned(),
    };
    let to_unicode = if font.has(b"ToUnicode") {
        "ToUnicodeあり（2バイトの文字コードでない）"
    } else {
        "ToUnicodeなし"
    };
    let object = doc
        .objects
        .iter()
        .find(|(_, object)| object.as_dict().is_ok_and(|d| std::ptr::eq(d, font)))
        .map(|((number, generation), _)| format!("、オブジェクト {number} {generation} R"))
        .unwrap_or_default();
    format!(
        "{}（BaseFont {base}、Encoding {encoding}、{to_unicode}{object}）",
        text(resource)
    )
}

impl<'a> PdfEncoding<'a> {
    fn for_font(font: &'a lopdf::Dictionary, doc: &'a Pdf) -> Result<Self> {
        let name = font
            .get_deref(b"Encoding", doc)
            .and_then(Object::as_name)
            .unwrap_or(b"");
        if let Some(ucs2) = unicode_cmap(name) {
            return Ok(Self::Unicode { ucs2 });
        }
        let encoding = font.get_font_encoding_with_limit(doc, MAX_PAGE)?;
        if font
            .get(b"Subtype")
            .and_then(Object::as_name)
            .is_ok_and(|n| n == b"Type0")
            && !matches!(encoding, lopdf::Encoding::UnicodeMapEncoding(_))
        {
            return Ok(Self::Unreadable);
        }
        Ok(Self::Mapped {
            encoding,
            width: code_width(font),
        })
    }

    fn decode(&self, bytes: &[u8]) -> Result<String> {
        match self {
            // Decoded code by code: lopdf reads a ToUnicode map greedily, so a code
            // missing from it would otherwise swallow the characters after it.
            Self::Mapped {
                encoding: encoding @ lopdf::Encoding::UnicodeMapEncoding(_),
                width,
            } => bytes
                .chunks(*width)
                .map(|code| Ok(Pdf::decode_text(encoding, code)?))
                .collect(),
            Self::Mapped { encoding, .. } => Ok(Pdf::decode_text(encoding, bytes)?),
            Self::Unreadable => bail!("PDF font codes cannot be read as text"),
            Self::Unicode { ucs2 } => {
                ensure!(
                    bytes.len().is_multiple_of(2),
                    "invalid PDF Unicode byte length"
                );
                let units: Vec<_> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect();
                ensure!(
                    !ucs2 || units.iter().all(|u| !(0xd800..=0xdfff).contains(u)),
                    "surrogate in PDF UCS2 text"
                );
                Ok(String::from_utf16(&units)?)
            }
        }
    }

    fn encode(&self, text: &str) -> Result<Vec<u8>> {
        match self {
            Self::Mapped { encoding, .. } => Ok(Pdf::encode_text(encoding, text)),
            Self::Unreadable => bail!("PDF font codes cannot be written from text"),
            Self::Unicode { ucs2 } => {
                ensure!(
                    !ucs2 || text.chars().all(|c| c as u32 <= 0xffff),
                    "replacement contains characters unavailable in PDF UCS2 encoding"
                );
                Ok(text.encode_utf16().flat_map(u16::to_be_bytes).collect())
            }
        }
    }
}

/// Identity of a font dictionary in a loaded document.
fn font_key(font: &lopdf::Dictionary) -> usize {
    std::ptr::from_ref(font) as usize
}

/// An embedded subset (`ABCDEF+Name`) holds only the glyphs its document uses.
fn subset_font(font: &lopdf::Dictionary) -> bool {
    font.get(b"BaseFont")
        .and_then(Object::as_name)
        .is_ok_and(|name| {
            name.len() > 7 && name[6] == b'+' && name[..6].iter().all(u8::is_ascii_uppercase)
        })
}

/// Bytes per character code: two for composite (Type0) fonts, one otherwise.
fn code_width(font: &lopdf::Dictionary) -> usize {
    if font
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|n| n == b"Type0")
    {
        2
    } else {
        1
    }
}

/// Character codes each subset font draws anywhere in the document; a subset
/// is known to hold a glyph only for these.
fn subset_glyphs(doc: &Pdf) -> Result<BTreeMap<usize, BTreeSet<Vec<u8>>>> {
    let mut used: BTreeMap<usize, BTreeSet<Vec<u8>>> = BTreeMap::new();
    for (_, id) in doc.get_pages() {
        let fonts = pdf_page_fonts(doc, id);
        let Ok(content) = Content::decode_strict(&doc.get_page_content_with_limit(id, MAX_PAGE)?)
        else {
            continue;
        };
        let mut font = None;
        for operation in &content.operations {
            match operation.operator.as_str() {
                "Tf" => {
                    font = operation
                        .operands
                        .first()
                        .and_then(|name| name.as_name().ok())
                        .and_then(|name| fonts.get(name))
                        .copied();
                }
                "Tj" | "TJ" | "'" | "\"" => {
                    let Some(current) = font.filter(|f| subset_font(f)) else {
                        continue;
                    };
                    let strings: Vec<&Object> = match operation.operands.last() {
                        Some(Object::Array(items)) => items.iter().collect(),
                        Some(other) => vec![other],
                        None => vec![],
                    };
                    let codes = used.entry(font_key(current)).or_default();
                    for object in strings {
                        if let Object::String(bytes, _) = object {
                            codes.extend(bytes.chunks(code_width(current)).map(<[u8]>::to_vec));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(used)
}

/// The graphics state of a PDF page that decides how text looks: the
/// transformation (`cm`), fill and stroke colours and the text font and size.
#[derive(Clone)]
struct PdfLook {
    matrix: [f64; 6],
    fill: Option<String>,
    stroke: Option<String>,
    fill_space: Vec<u8>,
    stroke_space: Vec<u8>,
    size: f64,
    /// The text state that moves the text as it is shown: character and
    /// word spacing (Tc, Tw), horizontal scaling as a fraction (Tz) and the
    /// leading between lines (TL).
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
}

impl Default for PdfLook {
    fn default() -> Self {
        // Colours start black in DeviceGray.
        Self {
            matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            fill: Some("000000".into()),
            stroke: Some("000000".into()),
            fill_space: b"DeviceGray".to_vec(),
            stroke_space: b"DeviceGray".to_vec(),
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
        }
    }
}

/// `a` then `b`, as PDF multiplies matrices.
fn pdf_multiply(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn pdf_numbers(operands: &[Object]) -> Vec<f64> {
    operands
        .iter()
        .filter_map(|o| o.as_float().ok().map(f64::from))
        .collect()
}

/// A colour of a device colour space (Gray, RGB or CMYK components) as
/// `RRGGBB`; colours of other spaces are not resolved.
fn pdf_color(space: &[u8], components: &[f64]) -> Option<String> {
    let channel = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let hex =
        |r: f64, g: f64, b: f64| format!("{:02X}{:02X}{:02X}", channel(r), channel(g), channel(b));
    match (space, components) {
        (b"DeviceGray" | b"G" | b"CalGray", [gray]) => Some(hex(*gray, *gray, *gray)),
        (b"DeviceRGB" | b"RGB" | b"CalRGB", [r, g, b]) => Some(hex(*r, *g, *b)),
        (b"DeviceCMYK" | b"CMYK", [c, m, y, k]) => Some(hex(
            (1.0 - c) * (1.0 - k),
            (1.0 - m) * (1.0 - k),
            (1.0 - y) * (1.0 - k),
        )),
        _ => None,
    }
}

/// The name of a PDF font (`BaseFont`) without the prefix of an embedded
/// subset (`ABCDEF+`); a name that is not UTF-8 is not given.
fn pdf_font_name(font: &lopdf::Dictionary) -> Option<String> {
    let name = font.get(b"BaseFont").and_then(Object::as_name).ok()?;
    let name = if subset_font(font) { &name[7..] } else { name };
    std::str::from_utf8(name)
        .ok()
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}

/// Decodes the page's text strings with the font each is drawn in, and with
/// `changes` replaces the strings at those indexes. `glyphs` (from
/// [`subset_glyphs`]) limits a replacement drawn with an embedded subset font
/// to the codes that subset is known to hold.
/// The fonts of the pages of `doc` naming both an /Encoding and a ToUnicode
/// map, without their /Encoding, by font dictionary: the ToUnicode map says
/// what their codes mean, where lopdf would otherwise use the /Encoding alone
/// (and replace it by StandardEncoding when it cannot read its glyph names).
/// These are the simple fonts, and the composite fonts whose CMap is neither
/// Identity nor a Unicode CMap ARP reads, when their ToUnicode map has
/// two-byte codes as ARP reads those of a composite font. A page whose fonts
/// cannot be read adds none; reading its text reports why.
fn pdf_mapped_fonts(doc: &Pdf) -> BTreeMap<usize, lopdf::Dictionary> {
    let mut mapped = BTreeMap::new();
    for id in doc.get_pages().into_values() {
        for font in pdf_page_fonts(doc, id).into_values() {
            let composite = code_width(font) == 2
                && font
                    .get_deref(b"Encoding", doc)
                    .and_then(Object::as_name)
                    .map_or(true, |name| {
                        !matches!(name, b"Identity-H" | b"Identity-V")
                            && unicode_cmap(name).is_none()
                    })
                && two_byte_to_unicode(font, doc);
            if (code_width(font) == 1 || composite)
                && font.has(b"ToUnicode")
                && font.has(b"Encoding")
            {
                mapped.entry(font_key(font)).or_insert_with(|| {
                    let mut font = font.clone();
                    font.remove(b"Encoding");
                    font
                });
            }
        }
    }
    mapped
}

/// The encodings of the fonts of a document, by font dictionary, each made
/// when a page first draws text in its font: reading one parses the font's
/// ToUnicode map, once for all pages rather than on every page.
struct PdfFonts<'a> {
    mapped: &'a BTreeMap<usize, lopdf::Dictionary>,
    encodings: BTreeMap<usize, PdfEncoding<'a>>,
    /// The fonts whose codes cannot be read, as [`unreadable_font`] names
    /// them, by font dictionary.
    unreadable: BTreeMap<usize, String>,
    /// The fonts whose strings [`pdf_text`] left out since this was last
    /// taken.
    left_out: BTreeSet<String>,
    /// The widths of the glyphs of each font, by font dictionary.
    widths: BTreeMap<usize, PdfWidths>,
}

impl<'a> PdfFonts<'a> {
    fn new(mapped: &'a BTreeMap<usize, lopdf::Dictionary>) -> Self {
        Self {
            mapped,
            encodings: BTreeMap::new(),
            unreadable: BTreeMap::new(),
            left_out: BTreeSet::new(),
            widths: BTreeMap::new(),
        }
    }
}

/// A string of a PDF page's text: its characters, its font (with its size
/// on the page), where it starts and how wide it runs (`[x, y, width]` in
/// the page's user space), whether it is written left to right along a
/// level line, whether the page draws it invisibly, and the number (MCID)
/// its content is marked with for the structure of a tagged PDF, if it is.
struct PdfString {
    text: String,
    font: crate::fonts::RunFont,
    origin: [f64; 3],
    upright: bool,
    invisible: bool,
    mark: Option<i64>,
}

/// The new characters of each string that changing the text of a PDF cell
/// made of `pieces` to `after` rewrites, by the string's index among the
/// page's `texts`; none when an export would refuse the change.
pub fn pdf_cell_strings(
    pieces: &[PdfPiece],
    texts: &[String],
    after: &str,
) -> Option<BTreeMap<usize, String>> {
    page_layout::distribute(pieces, |i| texts[i].as_str(), after).ok()
}

/// The sheet of the page `id`, which draws `content` and shows `strings`:
/// its rows, columns and tables, read from where the page places its
/// strings and rules its lines, and from the paragraphs and tables `tags`
/// names of it. The figures the page draws are found among its paths first,
/// from the tables its rulings and text would make, and the lines of a
/// figure then rule no table; those first tables are given too, by where
/// each lies on the page, as the figures are found from them, and those
/// among them that are tables beyond doubt.
fn pdf_page_layout(
    doc: &Pdf,
    id: lopdf::ObjectId,
    content: &Content,
    strings: &[PdfString],
    tags: &pdf_tags::Tags,
) -> (page_layout::Layout, Vec<[f64; 4]>, Vec<[f64; 4]>) {
    let [x0, y0, x1, y1] = pdf_page_box(doc, id);
    let tags = tags.page(id);
    let shown: Vec<page_layout::Shown<'_>> = strings
        .iter()
        .map(|string| page_layout::Shown {
            text: &string.text,
            x: string.origin[0] - x0,
            y: y1 - string.origin[1],
            width: string.origin[2],
            size: string.font.size.unwrap_or(0.0),
            upright: string.upright,
            invisible: string.invisible,
            unit: string.mark.and_then(|mark| tags?.units.get(&mark).copied()),
        })
        .collect();
    let paths = pdf_paths(content, [x0, y1], &pdf_pattern_colors(doc, id));
    let marked = tags.map_or(&[][..], |tags| &tags.tables);
    let sure = page_layout::sure_tables(&shown, &paths);
    let first = page_layout::layout(&shown, &paths, marked, &[], &sure);
    let figures = pdf_figures::regions(&paths, x1 - x0, y1 - y0, &first.table_bounds, &sure);
    if figures.is_empty() {
        let tables = first.table_bounds.clone();
        return (first, tables, sure);
    }
    (
        page_layout::layout(&shown, &paths, marked, &figures, &sure),
        first.table_bounds,
        sure,
    )
}

/// The page `id` of a PDF as [`Source::pdf_pages`] gives it, drawing the
/// images `placements`.
fn pdf_page<'a>(
    doc: &'a Pdf,
    id: lopdf::ObjectId,
    read: &mut PdfFonts<'a>,
    tags: &pdf_tags::Tags,
    placements: Vec<pdf_images::Placement>,
) -> Result<PdfPage> {
    let [x0, y0, x1, y1] = pdf_page_box(doc, id);
    let raw = doc.get_page_content_with_limit(id, MAX_PAGE)?;
    let (mut content, _) = pdf_content(&raw)?;
    let paths = pdf_paths(&content, [x0, y1], &pdf_pattern_colors(doc, id));
    let strings = pdf_text(doc, id, &mut content, &BTreeMap::new(), None, read)?;
    let (layout, tables, sure_tables) = pdf_page_layout(doc, id, &content, &strings, tags);
    let cells = layout.cells.into_iter().map(|cell| cell.pieces).collect();
    let mut places = vec![];
    let mut invisible = BTreeSet::new();
    let mut turned = BTreeSet::new();
    let mut texts = vec![];
    let mut fonts = vec![];
    for (i, string) in strings.into_iter().enumerate() {
        let [x, y, width] = string.origin;
        places.push([x - x0, y1 - y, width]);
        if string.invisible {
            invisible.insert(i);
        }
        if !string.upright {
            turned.insert(i);
        }
        fonts.push(crate::fonts::element_font(&[], &string.font));
        texts.push(string.text);
    }
    read.left_out.clear();
    // The unit square of an image, its top row at 1, onto the page from its
    // top left: SVG draws an image from its top left down.
    let images = placements
        .into_iter()
        .filter_map(|p| {
            let [a, b, c, d, e, f] = p.matrix?;
            Some(PdfImage {
                asset: pdf_images::stem(p.index),
                matrix: [a, -b, -c, d, c + e - x0, y1 - d - f],
                mask: p.mask,
            })
        })
        .collect();
    Ok(PdfPage {
        width: x1 - x0,
        height: y1 - y0,
        paths,
        places,
        texts,
        fonts,
        cells,
        invisible,
        turned,
        images,
        tables,
        sure_tables,
    })
}

/// The sheet of the `page`th page of a PDF as an extraction records it:
/// each cell of `layout` with the text and font of the `strings` it is made
/// of.
fn pdf_sheet(page: u32, strings: &[PdfString], layout: &page_layout::Layout) -> Value {
    let cells: Vec<Value> = layout
        .cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let mut value = String::new();
            let mut runs = vec![];
            for piece in &cell.pieces {
                let string = &strings[piece.string];
                value.push_str(piece.before);
                let start = value.chars().count();
                value.push_str(&string.text);
                runs.push(crate::fonts::TextRun {
                    font: string.font.clone(),
                    bold: false,
                    chars: start..value.chars().count(),
                });
            }
            // A cell of one string has that string's font.
            let font = match &runs[..] {
                [run] => crate::fonts::element_font(&[], &run.font),
                runs => crate::fonts::element_font(runs, &crate::fonts::RunFont::default()),
            };
            let column = crate::excel::column_name(cell.column).expect("a column of a sheet");
            json!({
                "id":format!("text-{}",i+1), "address":format!("{column}{}",cell.row),
                "type":"string", "value":value, "formula":null, "cached":null,
                "number_format":"", "font":font
            })
        })
        .collect();
    let mut sheet = json!({"name":format!("page-{page}"),"part":format!("pdf:{page}"),
        "state":"visible","merges":layout.merges,"tables":layout.tables});
    sheet["cells"] = Value::Array(cells);
    sheet
}

/// A page of a PDF original as [`Source::pdf_pages`] gives it.
pub struct PdfPage {
    pub width: f64,
    pub height: f64,
    /// The lines and areas the page paints, in the order it paints them.
    pub paths: Vec<PdfPath>,
    /// Where each string starts, and the width it runs along its line,
    /// `[x, y, width]` in points from the top left.
    pub places: Vec<[f64; 3]>,
    /// The characters of each string.
    pub texts: Vec<String>,
    /// The font of each string as an extraction records one.
    pub fonts: Vec<Value>,
    /// The strings each cell of the page's sheet is made of, in the order
    /// of the sheet's cells.
    pub cells: Vec<Vec<PdfPiece>>,
    /// The strings drawn invisibly (render modes 3 and 7), as the OCR text
    /// of a scanned page is, by their index in `places`.
    pub invisible: BTreeSet<usize>,
    /// The strings not written left to right along a level line, by their
    /// index in `places`.
    pub turned: BTreeSet<usize>,
    /// The images the page draws, in the order it draws them.
    pub images: Vec<PdfImage>,
    /// Where each table the page's rulings and text would make lies, the
    /// lines of its figures among them, `[x0, y0, x1, y1]` in points from
    /// the top left.
    pub tables: Vec<[f64; 4]>,
    /// Those of them that are tables beyond doubt: rulings drawn as lines
    /// whose cells hold text in columns and rows.
    pub sure_tables: Vec<[f64; 4]>,
}

/// An image a PDF page draws: the asset name of its file without the
/// extension, the SVG matrix mapping a unit square, drawn from its top left,
/// onto the page in points from the page's top left, and whether it is a
/// stencil mask, painting only its dark samples.
pub struct PdfImage {
    pub asset: String,
    pub matrix: [f64; 6],
    pub mask: bool,
}

/// A path a PDF page paints: its outline as SVG path data in points from
/// the page's top left, the colour that fills it and the colour and width
/// of the line drawn along it, each `RRGGBB`; none for what is not painted.
pub struct PdfPath {
    pub outline: String,
    pub fill: Option<String>,
    pub even_odd: bool,
    pub stroke: Option<String>,
    pub width: f64,
}

/// The colour of `components` in the colour space `space`: a device space,
/// else one of its number of components (a calibrated or ICC space of gray,
/// RGB or CMYK); a pattern or other space is not resolved.
fn pdf_any_color(space: &[u8], components: &[f64]) -> Option<String> {
    pdf_color(space, components).or_else(|| match components.len() {
        1 => pdf_color(b"DeviceGray", components),
        3 => pdf_color(b"DeviceRGB", components),
        4 => pdf_color(b"DeviceCMYK", components),
        _ => None,
    })
}

/// The graphics state that decides how a path is painted.
#[derive(Clone)]
struct PdfPaint {
    matrix: [f64; 6],
    fill: Option<String>,
    stroke: Option<String>,
    fill_space: Vec<u8>,
    stroke_space: Vec<u8>,
    width: f64,
}

/// The colour each shading pattern of the page `id` starts from, by the
/// pattern's name in the page's resources, as `RRGGBB`: what stands for a
/// gradient where only one colour can be painted. Tiling patterns and
/// shadings whose function or colour space cannot be read have none.
fn pdf_pattern_colors(doc: &Pdf, id: lopdf::ObjectId) -> BTreeMap<Vec<u8>, String> {
    let follow = |object: &'_ Object| -> Object {
        doc.dereference(object)
            .map_or_else(|_| object.clone(), |(_, o)| o.clone())
    };
    let dictionary = |object: &Object| -> Option<lopdf::Dictionary> {
        match follow(object) {
            Object::Dictionary(dict) => Some(dict),
            Object::Stream(stream) => Some(stream.dict),
            _ => None,
        }
    };
    // The values a function gives where its domain starts.
    fn start(doc: &Pdf, function: &Object, depth: usize) -> Option<Vec<f64>> {
        let function = doc.dereference(function).map_or(function, |(_, o)| o);
        if depth > 4 {
            return None;
        }
        let numbers = |dict: &lopdf::Dictionary, key: &[u8]| -> Option<Vec<f64>> {
            let array = dict.get(key).ok()?;
            let array = doc.dereference(array).map_or(array, |(_, o)| o);
            Some(pdf_numbers(array.as_array().ok()?))
        };
        match function {
            // A function for each component.
            Object::Array(functions) => functions
                .iter()
                .map(|f| start(doc, f, depth + 1)?.first().copied())
                .collect(),
            Object::Dictionary(dict) => match dict.get(b"FunctionType").and_then(Object::as_i64) {
                Ok(2) => Some(numbers(dict, b"C0").unwrap_or_else(|| vec![0.0])),
                Ok(3) => {
                    let functions = dict.get(b"Functions").ok()?;
                    let functions = doc.dereference(functions).map_or(functions, |(_, o)| o);
                    start(doc, functions.as_array().ok()?.first()?, depth + 1)
                }
                _ => None,
            },
            // Samples: the first of them, each as its range maps it.
            Object::Stream(stream) => {
                let dict = &stream.dict;
                if dict.get(b"FunctionType").and_then(Object::as_i64).ok()? != 0 {
                    return None;
                }
                let range = numbers(dict, b"Range")?;
                let bits = dict.get(b"BitsPerSample").and_then(Object::as_i64).ok()?;
                let bytes = match bits {
                    8 => 1,
                    16 => 2,
                    _ => return None,
                };
                let data = stream.decompressed_content_with_limit(MAX_PAGE).ok()?;
                let most = ((1u32 << bits) - 1) as f64;
                range
                    .chunks(2)
                    .enumerate()
                    .map(|(n, range)| {
                        let sample = data.get(n * bytes..(n + 1) * bytes)?;
                        let sample = sample.iter().fold(0u32, |v, b| v << 8 | u32::from(*b));
                        Some(range[0] + f64::from(sample) / most * (range.get(1)? - range[0]))
                    })
                    .collect()
            }
            _ => None,
        }
    }
    let mut colors = BTreeMap::new();
    for resources in pdf_page_resources(doc, id).into_iter().rev() {
        let Some(patterns) = resources.get(b"Pattern").ok().and_then(dictionary) else {
            continue;
        };
        for (name, pattern) in patterns.iter() {
            let color = dictionary(pattern)
                .and_then(|pattern| dictionary(pattern.get(b"Shading").ok()?))
                .and_then(|shading| {
                    let components = start(doc, shading.get(b"Function").ok()?, 0)?;
                    let space = match follow(shading.get(b"ColorSpace").ok()?) {
                        Object::Name(name) => name,
                        _ => vec![],
                    };
                    pdf_any_color(&space, &components)
                });
            match color {
                Some(color) => colors.insert(name.clone(), color),
                None => colors.remove(name),
            };
        }
    }
    colors
}

/// The paths `content` paints, its points taken from default user space to
/// the top left corner `[x0, y1]` of the page's box. Clipping, shadings and
/// images are left out; a path painted with a pattern is in the colour
/// `patterns` gives the pattern, and has no colour when they give none.
fn pdf_paths(
    content: &Content,
    [x0, y1]: [f64; 2],
    patterns: &BTreeMap<Vec<u8>, String>,
) -> Vec<PdfPath> {
    let mut state = PdfPaint {
        matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        fill: Some("000000".into()),
        stroke: Some("000000".into()),
        fill_space: b"DeviceGray".to_vec(),
        stroke_space: b"DeviceGray".to_vec(),
        width: 1.0,
    };
    let mut stack = vec![];
    let mut outline = String::new();
    let mut current = (0.0, 0.0);
    let mut paths = vec![];
    // Appends `command` with `points`, each on the page.
    let add = |outline: &mut String, matrix: &[f64; 6], command: char, points: &[(f64, f64)]| {
        outline.push(command);
        for (i, (x, y)) in points.iter().enumerate() {
            let px = matrix[0] * x + matrix[2] * y + matrix[4] - x0;
            let py = y1 - (matrix[1] * x + matrix[3] * y + matrix[5]);
            if i > 0 {
                outline.push(' ');
            }
            outline.push_str(&format!("{px:.2},{py:.2}"));
        }
    };
    for operation in &content.operations {
        let n = pdf_numbers(&operation.operands);
        let m = state.matrix;
        match (operation.operator.as_str(), n.as_slice()) {
            ("q", _) => stack.push(state.clone()),
            ("Q", _) => {
                if let Some(saved) = stack.pop() {
                    state = saved;
                }
            }
            ("cm", [a, b, c, d, e, f]) => {
                state.matrix = pdf_multiply([*a, *b, *c, *d, *e, *f], state.matrix);
            }
            ("w", [width]) => state.width = *width,
            ("g" | "rg" | "k", components) => {
                state.fill_space = match operation.operator.as_str() {
                    "g" => b"DeviceGray".to_vec(),
                    "rg" => b"DeviceRGB".to_vec(),
                    _ => b"DeviceCMYK".to_vec(),
                };
                state.fill = pdf_color(&state.fill_space, components);
            }
            ("G" | "RG" | "K", components) => {
                state.stroke_space = match operation.operator.as_str() {
                    "G" => b"DeviceGray".to_vec(),
                    "RG" => b"DeviceRGB".to_vec(),
                    _ => b"DeviceCMYK".to_vec(),
                };
                state.stroke = pdf_color(&state.stroke_space, components);
            }
            ("cs" | "CS", _) => {
                let space = operation
                    .operands
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .unwrap_or(b"")
                    .to_vec();
                if operation.operator == "cs" {
                    state.fill_space = space;
                } else {
                    state.stroke_space = space;
                }
            }
            ("sc" | "scn", components) if !components.is_empty() => {
                state.fill = pdf_any_color(&state.fill_space, components);
            }
            ("SC" | "SCN", components) if !components.is_empty() => {
                state.stroke = pdf_any_color(&state.stroke_space, components);
            }
            // A pattern alone (a shading, a coloured tiling) is painted in
            // the colour its shading starts from; one without is left
            // unpainted rather than in the colour set before it.
            ("scn" | "SCN", _) => {
                if let Some(Object::Name(name)) = operation.operands.last() {
                    let color = patterns.get(name).cloned();
                    if operation.operator == "scn" {
                        state.fill = color;
                    } else {
                        state.stroke = color;
                    }
                }
            }
            ("m", [x, y]) => {
                add(&mut outline, &m, 'M', &[(*x, *y)]);
                current = (*x, *y);
            }
            ("l", [x, y]) => {
                add(&mut outline, &m, 'L', &[(*x, *y)]);
                current = (*x, *y);
            }
            ("c", [a, b, c, d, x, y]) => {
                add(&mut outline, &m, 'C', &[(*a, *b), (*c, *d), (*x, *y)]);
                current = (*x, *y);
            }
            ("v", [c, d, x, y]) => {
                add(&mut outline, &m, 'C', &[current, (*c, *d), (*x, *y)]);
                current = (*x, *y);
            }
            ("y", [a, b, x, y]) => {
                add(&mut outline, &m, 'C', &[(*a, *b), (*x, *y), (*x, *y)]);
                current = (*x, *y);
            }
            ("h", _) => outline.push('Z'),
            ("re", [x, y, w, h]) => {
                add(&mut outline, &m, 'M', &[(*x, *y)]);
                add(
                    &mut outline,
                    &m,
                    'L',
                    &[(x + w, *y), (x + w, y + h), (*x, y + h)],
                );
                outline.push('Z');
                current = (*x, *y);
            }
            (paint @ ("S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n"), _) => {
                if matches!(paint, "s" | "b" | "b*") {
                    outline.push('Z');
                }
                let fills = matches!(paint, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                let strokes = matches!(paint, "S" | "s" | "B" | "B*" | "b" | "b*");
                if (fills || strokes) && !outline.is_empty() {
                    // The line width scales with the transformation; a width
                    // of 0 is the thinnest line a device draws.
                    let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
                    paths.push(PdfPath {
                        outline: std::mem::take(&mut outline),
                        fill: fills.then(|| state.fill.clone()).flatten(),
                        even_odd: paint.ends_with('*'),
                        stroke: strokes.then(|| state.stroke.clone()).flatten(),
                        width: (state.width * scale).max(0.25),
                    });
                }
                outline.clear();
            }
            _ => {}
        }
    }
    paths
}

/// The box of the page `id` that a viewer shows, `[x0, y0, x1, y1]`: its
/// crop box, else its media box, either inherited from the page tree; US
/// Letter when it has neither.
fn pdf_page_box(doc: &Pdf, id: lopdf::ObjectId) -> [f64; 4] {
    let rectangle = |dictionary: &lopdf::Dictionary, key: &[u8]| {
        let corners: Vec<f64> = dictionary
            .get_deref(key, doc)
            .and_then(Object::as_array)
            .ok()?
            .iter()
            .filter_map(|o| doc.dereference(o).ok()?.1.as_float().ok().map(f64::from))
            .collect();
        let [a, b, c, d] = corners[..] else {
            return None;
        };
        Some([a.min(c), b.min(d), a.max(c), b.max(d)])
    };
    for key in [b"CropBox".as_slice(), b"MediaBox"] {
        let mut node = doc.get_dictionary(id).ok();
        // A page tree is shallow; the bound keeps a cycle from looping.
        for _ in 0..64 {
            let Some(dictionary) = node else { break };
            if let Some(found) = rectangle(dictionary, key) {
                return found;
            }
            node = dictionary
                .get(b"Parent")
                .and_then(Object::as_reference)
                .and_then(|parent| doc.get_dictionary(parent))
                .ok();
        }
    }
    [0.0, 0.0, 612.0, 792.0]
}

/// The advance widths of a font's glyphs, in thousandths of the text size,
/// by character code: how far showing a string moves the text position.
/// A simple font lists them from its first code (/FirstChar, /Widths), a
/// composite font by CID ranges (/W) with a default (/DW); a code without a
/// width takes the font's missing width, or half the size for a standard
/// font, which lists none.
struct PdfWidths {
    /// The bytes of a code: 2 for a composite font.
    code: usize,
    first: i64,
    simple: Vec<f64>,
    composite: BTreeMap<u32, f64>,
    default: f64,
}

impl PdfWidths {
    fn for_font(font: &lopdf::Dictionary, doc: &Pdf) -> Self {
        let number = |object: &Object| {
            doc.dereference(object)
                .ok()
                .and_then(|(_, o)| o.as_float().ok())
                .map(f64::from)
        };
        let array = |dictionary: &lopdf::Dictionary, key: &[u8]| {
            dictionary
                .get_deref(key, doc)
                .and_then(Object::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let code = code_width(font);
        let mut widths = Self {
            code,
            first: 0,
            simple: vec![],
            composite: BTreeMap::new(),
            default: 500.0,
        };
        if code == 2 {
            let Some(descendant) = font
                .get_deref(b"DescendantFonts", doc)
                .and_then(Object::as_array)
                .ok()
                .and_then(|fonts| fonts.first())
                .and_then(|f| doc.dereference(f).ok())
                .and_then(|(_, f)| f.as_dict().ok())
            else {
                widths.default = 1000.0;
                return widths;
            };
            widths.default = descendant
                .get(b"DW")
                .ok()
                .and_then(number)
                .unwrap_or(1000.0);
            // Entries are `c [w1 w2 …]` or `first last w`.
            let entries = array(descendant, b"W");
            let mut index = 0;
            while index + 1 < entries.len() {
                let Some(first) = number(&entries[index]) else {
                    break;
                };
                let first = first as u32;
                match doc.dereference(&entries[index + 1]).map(|(_, o)| o) {
                    Ok(Object::Array(list)) => {
                        for (offset, width) in list.iter().enumerate() {
                            if let Some(width) = number(width) {
                                widths.composite.insert(first + offset as u32, width);
                            }
                        }
                        index += 2;
                    }
                    _ => {
                        let (Some(last), Some(width)) = (
                            number(&entries[index + 1]),
                            entries.get(index + 2).and_then(number),
                        ) else {
                            break;
                        };
                        for cid in first..=(last as u32).min(first.saturating_add(65_535)) {
                            widths.composite.insert(cid, width);
                        }
                        index += 3;
                    }
                }
            }
        } else {
            // A Type3 font's widths are in its glyph space, which its font
            // matrix maps onto text space; other fonts' are thousandths.
            let scale = font
                .get_deref(b"FontMatrix", doc)
                .and_then(Object::as_array)
                .ok()
                .filter(|_| {
                    font.get(b"Subtype")
                        .and_then(Object::as_name)
                        .is_ok_and(|s| s == b"Type3")
                })
                .and_then(|m| m.first())
                .and_then(number)
                .map_or(1.0, |a| a * 1000.0);
            widths.first = font.get(b"FirstChar").ok().and_then(number).unwrap_or(0.0) as i64;
            widths.simple = array(font, b"Widths")
                .iter()
                .filter_map(number)
                .map(|w| w * scale)
                .collect();
            if let Some(missing) = font
                .get_deref(b"FontDescriptor", doc)
                .and_then(Object::as_dict)
                .ok()
                .and_then(|d| d.get(b"MissingWidth").ok())
                .and_then(number)
                .filter(|w| *w > 0.0)
            {
                widths.default = missing;
            }
        }
        widths
    }

    fn advance(&self, code: &[u8]) -> f64 {
        if self.code == 2 {
            let cid = code.iter().fold(0u32, |cid, b| cid << 8 | u32::from(*b));
            return self.composite.get(&cid).copied().unwrap_or(self.default);
        }
        let index = i64::from(code[0]) - self.first;
        usize::try_from(index)
            .ok()
            .and_then(|i| self.simple.get(i))
            .copied()
            .unwrap_or(self.default)
    }
}

/// The strings of the text a page shows, in the order it shows them, each
/// with its font, where it starts on the page (in default user space) and
/// how far along its line it runs: those of its content, with those of the
/// Form XObjects it draws where it draws them, then those of the
/// appearances of its annotations other than buttons. Only strings of the
/// page's content are rewritten by `changes`; a change to another is
/// refused.
fn pdf_text<'a>(
    doc: &'a Pdf,
    page: lopdf::ObjectId,
    content: &mut Content<Vec<lopdf::content::Operation>>,
    changes: &BTreeMap<usize, String>,
    glyphs: Option<&BTreeMap<usize, BTreeSet<Vec<u8>>>>,
    read: &mut PdfFonts<'a>,
) -> Result<Vec<PdfString>> {
    let fonts = pdf_page_fonts(doc, page);
    let resources = pdf_page_resources(doc, page);
    let mut texts = PdfTexts {
        doc,
        changes,
        glyphs,
        read,
        values: vec![],
        open: BTreeSet::new(),
        refused: None,
    };
    texts.content(
        content,
        &fonts,
        &resources,
        PdfTextState::default(),
        None,
        0,
    )?;
    // An appearance without resources of its own, as many a form field's
    // is written, uses the form's default resources after the page's, as
    // PDF readers let it.
    let dictionary = |object: &'a Object| match object {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        object => object.as_dict().ok(),
    };
    let mut shown_with = resources.clone();
    shown_with.extend(
        doc.catalog()
            .ok()
            .and_then(|catalog| dictionary(catalog.get(b"AcroForm").ok()?))
            .and_then(|form| dictionary(form.get(b"DR").ok()?)),
    );
    let fonts = pdf_resource_fonts(doc, &shown_with);
    for (appearance, onto) in pdf_images::appearances(doc, page)
        .into_iter()
        .filter(|(_, _, button)| !button)
        .map(|(appearance, onto, _)| (appearance, onto))
    {
        let state = PdfTextState {
            look: PdfLook {
                matrix: onto,
                ..PdfLook::default()
            },
            ..PdfTextState::default()
        };
        texts.form(appearance, &fonts, &shown_with, state, "an annotation", 0);
    }
    if let Some(refused) = texts.refused {
        return Err(refused);
    }
    ensure!(
        changes.keys().all(|i| *i < texts.values.len()),
        "PDF text target missing"
    );
    Ok(texts.values)
}

/// The graphics state text is shown in, which a Form XObject inherits from
/// what draws it: the font (by its resource name), the text render mode
/// (Tr: 3 draws nothing and 7 only clips, as in the OCR layer of a scanned
/// page), the look, and the number (MCID) the page marks the content being
/// drawn with for its structure.
#[derive(Clone, Default)]
struct PdfTextState {
    font: Vec<u8>,
    render: i64,
    look: PdfLook,
    mark: Option<i64>,
}

/// Reads the strings a page shows, as [`pdf_text`] does.
struct PdfTexts<'a, 'b> {
    doc: &'a Pdf,
    changes: &'b BTreeMap<usize, String>,
    glyphs: Option<&'b BTreeMap<usize, BTreeSet<Vec<u8>>>>,
    read: &'b mut PdfFonts<'a>,
    values: Vec<PdfString>,
    /// The forms being read, which may not draw themselves.
    open: BTreeSet<lopdf::ObjectId>,
    /// Why a change to a string outside the page's content is refused.
    refused: Option<anyhow::Error>,
}

/// How deep Form XObjects may draw one another for their text.
const MAX_TEXT_FORM_DEPTH: usize = 16;

impl<'a> PdfTexts<'a, '_> {
    /// The strings of the Form XObject (or appearance) `id`, drawn in
    /// `state` by what has `fonts` and `resources`. A form that cannot be
    /// read gives none, the same whenever the page is read, so that the
    /// strings after it keep their places.
    fn form(
        &mut self,
        id: lopdf::ObjectId,
        fonts: &BTreeMap<Vec<u8>, &'a lopdf::Dictionary>,
        resources: &[&'a lopdf::Dictionary],
        mut state: PdfTextState,
        inside: &'static str,
        depth: usize,
    ) {
        let doc = self.doc;
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
            return;
        };
        if depth >= MAX_TEXT_FORM_DEPTH || !self.open.insert(id) {
            return;
        }
        let read = stream
            .decompressed_content_with_limit(MAX_PAGE)
            .ok()
            .and_then(|raw| pdf_content(&raw).ok());
        if let Some((mut content, _)) = read {
            // A form without resources of its own uses those of what draws it.
            let own = stream.dict.get(b"Resources").ok().and_then(|r| match r {
                Object::Reference(id) => doc.get_dictionary(*id).ok(),
                r => r.as_dict().ok(),
            });
            let (fonts, resources) = match own {
                Some(own) => (pdf_resource_fonts(doc, &[own]), vec![own]),
                None => (fonts.clone(), resources.to_vec()),
            };
            let matrix = stream
                .dict
                .get(b"Matrix")
                .ok()
                .and_then(|m| doc.dereference(m).ok())
                .and_then(|(_, m)| m.as_array().ok())
                .map(|m| pdf_numbers(m))
                .and_then(|m| <[f64; 6]>::try_from(m).ok())
                .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
            state.look.matrix = pdf_multiply(matrix, state.look.matrix);
            let mark = self.values.len();
            if self
                .content(
                    &mut content,
                    &fonts,
                    &resources,
                    state,
                    Some(inside),
                    depth + 1,
                )
                .is_err()
            {
                self.values.truncate(mark);
            }
        }
        self.open.remove(&id);
    }

    /// The strings of `content`, drawn in `state` with `fonts` and
    /// `resources`; `inside` names what draws them when they are not the
    /// page's content, which is not rewritten.
    fn content(
        &mut self,
        content: &mut Content<Vec<lopdf::content::Operation>>,
        fonts: &BTreeMap<Vec<u8>, &'a lopdf::Dictionary>,
        resources: &[&'a lopdf::Dictionary],
        state: PdfTextState,
        inside: Option<&'static str>,
        depth: usize,
    ) -> Result<()> {
        const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let doc = self.doc;
        let changes = self.changes;
        let glyphs = self.glyphs;
        let PdfTextState {
            mut font,
            mut render,
            mut look,
            mut mark,
        } = state;
        // The marks of the marked content sequences open in this content.
        let mut marks: Vec<Option<i64>> = vec![];
        let mut text_matrix = IDENTITY;
        // The start of the current line, which Td, TD and T* move from.
        let mut line_matrix = IDENTITY;
        let mut stack = vec![];
        for operation in &mut content.operations {
            let numbers = pdf_numbers(&operation.operands);
            // ' and " move to the next line before they show their string.
            let next_line = |look: &PdfLook, line_matrix: &mut [f64; 6]| {
                *line_matrix = pdf_multiply([1.0, 0.0, 0.0, 1.0, 0.0, -look.leading], *line_matrix);
                *line_matrix
            };
            match operation.operator.as_str() {
                "cm" if numbers.len() == 6 => {
                    let m = [
                        numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
                    ];
                    look.matrix = pdf_multiply(m, look.matrix);
                }
                "BT" => (text_matrix, line_matrix) = (IDENTITY, IDENTITY),
                "Tm" if numbers.len() == 6 => {
                    text_matrix = [
                        numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
                    ];
                    line_matrix = text_matrix;
                }
                "Td" | "TD" if numbers.len() == 2 => {
                    if operation.operator == "TD" {
                        look.leading = -numbers[1];
                    }
                    line_matrix =
                        pdf_multiply([1.0, 0.0, 0.0, 1.0, numbers[0], numbers[1]], line_matrix);
                    text_matrix = line_matrix;
                }
                "T*" => text_matrix = next_line(&look, &mut line_matrix),
                "Tc" if numbers.len() == 1 => look.char_spacing = numbers[0],
                "Tw" if numbers.len() == 1 => look.word_spacing = numbers[0],
                "Tz" if numbers.len() == 1 => look.scale = numbers[0] / 100.0,
                "TL" if numbers.len() == 1 => look.leading = numbers[0],
                "g" | "rg" | "k" => {
                    look.fill_space = match operation.operator.as_str() {
                        "g" => b"DeviceGray".to_vec(),
                        "rg" => b"DeviceRGB".to_vec(),
                        _ => b"DeviceCMYK".to_vec(),
                    };
                    look.fill = pdf_color(&look.fill_space, &numbers);
                }
                "G" | "RG" | "K" => {
                    look.stroke_space = match operation.operator.as_str() {
                        "G" => b"DeviceGray".to_vec(),
                        "RG" => b"DeviceRGB".to_vec(),
                        _ => b"DeviceCMYK".to_vec(),
                    };
                    look.stroke = pdf_color(&look.stroke_space, &numbers);
                }
                "cs" | "CS" => {
                    let space = operation
                        .operands
                        .first()
                        .and_then(|o| o.as_name().ok())
                        .unwrap_or(b"")
                        .to_vec();
                    // A new colour space starts at its initial colour; device spaces start black.
                    let black = match space.as_slice() {
                        b"DeviceGray" | b"G" | b"CalGray" => pdf_color(&space, &[0.0]),
                        b"DeviceRGB" | b"RGB" | b"CalRGB" => pdf_color(&space, &[0.0, 0.0, 0.0]),
                        b"DeviceCMYK" | b"CMYK" => pdf_color(&space, &[0.0, 0.0, 0.0, 1.0]),
                        _ => None,
                    };
                    if operation.operator == "cs" {
                        look.fill_space = space;
                        look.fill = black;
                    } else {
                        look.stroke_space = space;
                        look.stroke = black;
                    }
                }
                "sc" | "scn" => look.fill = pdf_color(&look.fill_space, &numbers),
                "SC" | "SCN" => look.stroke = pdf_color(&look.stroke_space, &numbers),
                "Tf" => {
                    font = operation
                        .operands
                        .first()
                        .context("missing PDF font")?
                        .as_name()?
                        .to_vec();
                    look.size = numbers.last().copied().unwrap_or(0.0);
                }
                "Tr" => {
                    render = operation
                        .operands
                        .first()
                        .and_then(|mode| mode.as_i64().ok())
                        .unwrap_or(0);
                }
                // Marked content of the page: a sequence numbered for the
                // structure tree is of the element naming that number, an
                // artifact (a running head, a page number) of none. What a
                // form marks is numbered for the form, not the page.
                "BMC" | "BDC" => {
                    marks.push(mark);
                    let tag = operation
                        .operands
                        .first()
                        .and_then(|tag| tag.as_name().ok())
                        .unwrap_or(b"");
                    if inside.is_some() {
                    } else if tag == b"Artifact" {
                        mark = None;
                    } else if let Some(properties) = operation.operands.get(1) {
                        let properties = match properties {
                            Object::Name(name) => resources.iter().find_map(|r| {
                                let named = match r.get(b"Properties").ok()? {
                                    Object::Reference(id) => doc.get_dictionary(*id).ok()?,
                                    named => named.as_dict().ok()?,
                                };
                                match named.get(name).ok()? {
                                    Object::Reference(id) => doc.get_dictionary(*id).ok(),
                                    properties => properties.as_dict().ok(),
                                }
                            }),
                            properties => properties.as_dict().ok(),
                        };
                        if let Some(id) = properties
                            .and_then(|p| p.get(b"MCID").ok())
                            .and_then(|id| id.as_i64().ok())
                        {
                            mark = Some(id);
                        }
                    }
                }
                "EMC" => {
                    if let Some(outer) = marks.pop() {
                        mark = outer;
                    }
                }
                "q" => stack.push((font.clone(), render, look.clone())),
                // A Q without its q restores nothing, as PDF readers take it.
                "Q" => {
                    if let Some(saved) = stack.pop() {
                        (font, render, look) = saved;
                    }
                }
                "Do" => {
                    let Some(id) = operation
                        .operands
                        .first()
                        .and_then(|name| name.as_name().ok())
                        .and_then(|name| {
                            resources.iter().find_map(|r| {
                                let named = match r.get(b"XObject").ok()? {
                                    Object::Reference(id) => doc.get_dictionary(*id).ok()?,
                                    named => named.as_dict().ok()?,
                                };
                                named.get(name).ok()?.as_reference().ok()
                            })
                        })
                    else {
                        continue;
                    };
                    let form = doc
                        .get_object(id)
                        .and_then(Object::as_stream)
                        .is_ok_and(|s| {
                            s.dict
                                .get(b"Subtype")
                                .and_then(Object::as_name)
                                .is_ok_and(|s| s == b"Form")
                        });
                    if form {
                        let state = PdfTextState {
                            font: font.clone(),
                            render,
                            look: look.clone(),
                            mark,
                        };
                        self.form(id, fonts, resources, state, "a Form XObject", depth);
                    }
                }
                "Tj" | "TJ" | "'" | "\"" => {
                    if operation.operator == "\"" && numbers.len() >= 2 {
                        look.word_spacing = numbers[0];
                        look.char_spacing = numbers[1];
                    }
                    if matches!(operation.operator.as_str(), "'" | "\"") {
                        text_matrix = next_line(&look, &mut line_matrix);
                    }
                    let operand = operation
                        .operands
                        .last_mut()
                        .context("missing PDF text operand")?;
                    // The strings and the adjustments between them (TJ), in order.
                    let strings: Vec<&mut Object> = match operand {
                        Object::Array(items) => items
                            .iter_mut()
                            .filter(|o| matches!(o, Object::String(..)) || o.as_float().is_ok())
                            .collect(),
                        Object::String(..) => vec![operand],
                        _ => bail!("invalid PDF text operand"),
                    };
                    let font_dictionary = *fonts.get(&font).context("PDF text font not found")?;
                    let key = font_key(font_dictionary);
                    if !self.read.encodings.contains_key(&key) {
                        let encoding = PdfEncoding::for_font(
                            self.read.mapped.get(&key).unwrap_or(font_dictionary),
                            doc,
                        )?;
                        if matches!(encoding, PdfEncoding::Unreadable) {
                            self.read
                                .unreadable
                                .insert(key, unreadable_font(doc, &font, font_dictionary));
                        }
                        self.read.encodings.insert(key, encoding);
                    }
                    // Text in a font whose codes say no characters is left out,
                    // and kept as it is when the page is written.
                    if let Some(name) = self.read.unreadable.get(&key) {
                        self.read.left_out.insert(name.clone());
                        continue;
                    }
                    let encoding = &self.read.encodings[&key];
                    // The size on the page: the font size scaled by the text and
                    // transformation matrices; the colour of what the render mode draws.
                    let shown = pdf_multiply(text_matrix, look.matrix);
                    let size = look.size.abs() * (shown[2] * shown[2] + shown[3] * shown[3]).sqrt();
                    let name = pdf_font_name(font_dictionary);
                    let color = match render {
                        1 | 5 => look.stroke.clone(),
                        3 | 7 => None,
                        _ => look.fill.clone(),
                    };
                    let look_of = crate::fonts::RunFont {
                        latin: name.clone(),
                        east_asian: name,
                        size: (size > 0.0).then_some(size),
                        color: color.map(crate::fonts::Color::rgb),
                        ..crate::fonts::RunFont::default()
                    };
                    let widths = self
                        .read
                        .widths
                        .entry(key)
                        .or_insert_with(|| PdfWidths::for_font(font_dictionary, doc));
                    for object in strings {
                        let Object::String(bytes, _) = object else {
                            // A number of a TJ array moves the next string back
                            // by thousandths of the text size.
                            let adjustment = f64::from(object.as_float()?);
                            let tx = -adjustment / 1000.0 * look.size * look.scale;
                            text_matrix = pdf_multiply([1.0, 0.0, 0.0, 1.0, tx, 0.0], text_matrix);
                            continue;
                        };
                        let start = pdf_multiply(text_matrix, look.matrix);
                        // Each code moves the text by its glyph's width, with the
                        // character spacing and, for a single-byte space, the word spacing.
                        let tx: f64 = bytes
                            .chunks(widths.code)
                            .map(|code| {
                                let space = if code == b" " { look.word_spacing } else { 0.0 };
                                (widths.advance(code) / 1000.0 * look.size
                                    + look.char_spacing
                                    + space)
                                    * look.scale
                            })
                            .sum();
                        text_matrix = pdf_multiply([1.0, 0.0, 0.0, 1.0, tx, 0.0], text_matrix);
                        let origin = [start[4], start[5], tx * start[0].hypot(start[1])];
                        let value = encoding
                            .decode(bytes)
                            .context("PDF font encoding cannot be decoded")?;
                        let replacement = changes.get(&self.values.len());
                        if let (Some(_), Some(inside)) = (replacement, inside) {
                            // Refused once the page is read, so that the strings
                            // keep their places whatever stops reading a form.
                            self.refused.get_or_insert_with(|| anyhow::anyhow!(
                            "PDF text {value:?} is drawn by {inside}, not the page's own content, which is all ARP rewrites; edit it in a PDF editor"
                        ));
                        } else if let Some(replacement) = replacement {
                            ensure!(
                                !matches!(render, 3 | 7),
                                "PDF text {value:?} is drawn invisibly (such as the OCR text layer of a scanned page), so an edit would not change what the page shows; edit it in a PDF editor"
                            );
                            ensure!(
                                !replacement.contains(['\n', '\r', '\t']),
                                "PDF text replacement cannot contain line breaks or tabs; layout operations are not supported"
                            );
                            // The string must be what its codes say: codes the encoding
                            // drops, or several codes for one character (such as the
                            // vertical forms of a Japanese font), would change on re-encoding.
                            ensure!(
                                encoding.encode(&value)? == *bytes,
                                "PDF text {value:?} uses character codes its font does not map one-to-one to text, so rewriting it would change the unedited characters; edit it in a PDF editor"
                            );
                            let encoded = encoding.encode(replacement)?;
                            ensure!(
                                encoding.decode(&encoded)? == *replacement,
                                "replacement contains characters unavailable in the original PDF font encoding"
                            );
                            if let Some(glyphs) = glyphs
                                && subset_font(font_dictionary)
                            {
                                let known = glyphs.get(&font_key(font_dictionary));
                                for code in encoded.chunks(code_width(font_dictionary)) {
                                    let text = encoding.decode(code).unwrap_or_default();
                                    ensure!(
                                        known.is_some_and(|codes| codes.contains(code)),
                                        "the embedded font subset has no known glyph for {text:?}, which the document never draws in that font; use characters the PDF already shows in that text, or edit it in a PDF editor"
                                    );
                                }
                            }
                            *bytes = encoded;
                        }
                        self.values.push(PdfString {
                            text: value,
                            font: look_of.clone(),
                            origin,
                            // Along a level line, not turned or mirrored.
                            upright: start[0] > 0.0
                                && start[1].abs() <= 0.02 * start[0]
                                && start[3] > 0.0,
                            invisible: matches!(render, 3 | 7),
                            mark,
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Write};
    use zip::{ZipWriter, write::SimpleFileOptions};

    /// The canonical XML of the first element named `name` in `xml`.
    fn canonical(xml: &str, name: &str) -> String {
        let doc = Document::parse(xml).unwrap();
        let node = doc
            .descendants()
            .find(|n| n.tag_name().name() == name)
            .unwrap();
        let mut out = String::new();
        canonical_xml(node, &mut out);
        out
    }

    #[test]
    fn print_settings_read_the_same_whichever_program_saved_them() {
        const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
        const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let excel = format!(
            r#"<worksheet xmlns="{MAIN}" xmlns:r="{REL}"><pageSetUpPr/><pageSetup paperSize="9" orientation="landscape" r:id="rId1"/><headerFooter><oddHeader>&amp;C&amp;"ＭＳ ゴシック"見出し &lt;1&gt;</oddHeader></headerFooter></worksheet>"#
        );
        let other = format!(
            r#"<x:worksheet xmlns:x="{MAIN}" xmlns:rel="{REL}">
  <x:pageSetUpPr />
  <x:pageSetup rel:id="rId1" orientation='landscape' paperSize="9" />
  <x:headerFooter>
    <x:oddHeader>&amp;C&amp;"ＭＳ ゴシック"見出し &lt;1></x:oddHeader>
  </x:headerFooter>
</x:worksheet>"#
        );
        assert_eq!(canonical(&excel, "pageSetUpPr"), "<pageSetUpPr/>");
        assert_eq!(
            canonical(&excel, "pageSetup"),
            r#"<pageSetup orientation="landscape" paperSize="9" r:id="rId1"/>"#
        );
        assert_eq!(
            canonical(&excel, "headerFooter"),
            r#"<headerFooter><oddHeader>&amp;C&amp;"ＭＳ ゴシック"見出し &lt;1&gt;</oddHeader></headerFooter>"#
        );
        // Spacing, attribute order and quotes do not show; the prefixes the
        // writer declared do.
        assert_eq!(canonical(&other, "pageSetUpPr"), "<x:pageSetUpPr/>");
        assert_eq!(
            canonical(&other, "headerFooter"),
            r#"<x:headerFooter><x:oddHeader>&amp;C&amp;"ＭＳ ゴシック"見出し &lt;1&gt;</x:oddHeader></x:headerFooter>"#
        );
    }

    #[test]
    fn office_parts_keep_binary_names_without_retaining_bytes() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<document/>").unwrap();
        writer
            .start_file("word/media/image1.png", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"image bytes").unwrap();
        let raw = writer.finish().unwrap().into_inner();
        let selected = office_parts(&raw, false).unwrap();
        assert_eq!(selected["word/document.xml"], b"<document/>");
        assert!(selected["word/media/image1.png"].is_empty());
        assert_eq!(
            office_parts(&raw, true).unwrap()["word/media/image1.png"],
            b"image bytes"
        );
    }

    #[test]
    fn skipped_binary_crc_is_checked_before_writeback() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "word/media/image1.png",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"image bytes").unwrap();
        let mut raw = writer.finish().unwrap().into_inner();
        let offset = raw
            .windows(b"image bytes".len())
            .position(|window| window == b"image bytes")
            .unwrap();
        raw[offset] ^= 1;
        assert!(office_parts(&raw, false).is_ok());
        assert!(validate_office_binary(&raw).is_err());
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_office_parts() {
        use std::time::Instant;

        let media = vec![0x5a; 32 * 1024 * 1024];
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<document/>").unwrap();
        writer
            .start_file("word/media/image1.png", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&media).unwrap();
        let raw = writer.finish().unwrap().into_inner();
        for include_binary in [true, false] {
            let start = Instant::now();
            let parts = office_parts(&raw, include_binary).unwrap();
            let retained: usize = parts.values().map(Vec::len).sum();
            eprintln!(
                "office_parts include_binary={include_binary} elapsed_ms={} retained_bytes={retained}",
                start.elapsed().as_millis()
            );
        }
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_bound_controls() {
        use std::time::Instant;

        let body = (0..500)
            .map(|index| format!("<w:sdt><w:sdtContent><w:p><w:r><w:t>{index}</w:t></w:r></w:p></w:sdtContent></w:sdt>"))
            .collect::<String>();
        let xml = format!("<w:document xmlns:w=\"{WORD}\"><w:body>{body}</w:body></w:document>");
        let doc = Document::parse(&xml).unwrap();
        let edits: Vec<_> = doc
            .descendants()
            .filter(|node| node.has_tag_name((WORD, "t")))
            .map(|node| (node, "replacement".to_owned()))
            .collect();
        let controls: Vec<_> = doc
            .descendants()
            .filter(|node| node.has_tag_name((WORD, "sdt")))
            .collect();
        let start = Instant::now();
        let old: Vec<_> = controls
            .iter()
            .map(|control| {
                let mut indexed = BTreeMap::new();
                for (node, value) in &edits {
                    indexed.entry(node.range().start).or_insert(value.as_str());
                }
                word::control_text(*control, &indexed)
            })
            .collect();
        let old_ms = start.elapsed().as_millis();
        let start = Instant::now();
        let mut indexed = BTreeMap::new();
        for (node, value) in &edits {
            indexed.entry(node.range().start).or_insert(value.as_str());
        }
        let new: Vec<_> = controls
            .iter()
            .map(|control| word::control_text(*control, &indexed))
            .collect();
        let new_ms = start.elapsed().as_millis();
        assert_eq!(old, new);
        eprintln!(
            "bound_controls old_ms={old_ms} new_ms={new_ms} controls={} edits={}",
            controls.len(),
            edits.len()
        );
    }

    const BODY: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
        <w:p><w:r><w:t>plain</w:t></w:r></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>DATE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>2026/09/25</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>after</w:t></w:r></w:p>
        <w:p><w:fldSimple w:instr="PAGE"><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>TOC</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>
        <w:p><w:r><w:t>entry</w:t></w:r></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>tail</w:t></w:r></w:p>
        </w:body></w:document>"#;

    /// Names, strings and comments holding `BI` are not images; a known data
    /// length ends an image even when its data holds ` EI `.
    #[test]
    fn inline_images_are_found_outside_names_strings_and_comments() {
        let raw = b"/BI 1 gs % BI\n(BI ID EI) Tj BI /W 4 /H 1 /BPC 8 /CS /G ID  EI  EI Q BI /F /AHx ID 00> EI";
        let text = |ranges: Vec<std::ops::Range<usize>>| -> Vec<String> {
            ranges
                .into_iter()
                .map(|r| String::from_utf8_lossy(&raw[r]).into_owned())
                .collect()
        };
        assert_eq!(
            text(inline_images(raw, &[Some(4), None]).unwrap()),
            [
                "BI /W 4 /H 1 /BPC 8 /CS /G ID  EI  EI",
                "BI /F /AHx ID 00> EI"
            ]
        );
        // Without lopdf's length, the dictionary gives it.
        assert_eq!(
            text(inline_images(raw, &[None, None]).unwrap())[0],
            "BI /W 4 /H 1 /BPC 8 /CS /G ID  EI  EI"
        );
        // A color space named in the resources does not: the first EI set
        // off by whitespace ends the data.
        let named = b"BI /W 4 /H 1 /BPC 8 /CS /CS0 ID  EI  EI";
        let ranges = inline_images(named, &[None]).unwrap();
        assert_eq!(
            &named[ranges[0].clone()],
            b"BI /W 4 /H 1 /BPC 8 /CS /CS0 ID  EI"
        );
    }

    #[test]
    fn word_field_results_cover_complex_simple_and_multi_paragraph_fields() {
        let xml = Document::parse(BODY).unwrap();
        let results = word_field_results(&xml);
        let flags: Vec<_> = xml
            .descendants()
            .filter(|n| n.has_tag_name((WORD, "t")))
            .map(|n| results.contains(&n.range().start))
            .collect();
        assert_eq!(flags, [false, true, false, true, true, false]);
    }

    fn package(path: &Path, parts: &[(&str, Vec<u8>)]) {
        let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
        for (name, content) in parts {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }

    fn word_package(path: &Path, body: &str, extra: &[(&str, Vec<u8>)]) {
        let mut parts = vec![
            (
                "_rels/.rels",
                format!(
                    r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/officeDocument" Target="word/document.xml"/></Relationships>"#
                )
                .into_bytes(),
            ),
            ("word/document.xml", body.as_bytes().to_vec()),
        ];
        parts.extend(extra.iter().cloned());
        package(path, &parts);
    }

    #[test]
    fn word_images_and_field_codes_are_importable_without_changing_body_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("illustrated.docx");
        word_package(
            &path,
            BODY,
            &[("word/media/image1.png", b"image bytes".to_vec())],
        );
        let source = Source::open(&path).unwrap();
        assert_eq!(
            source
                .image_assets(&crate::project::AssetLimits::default())
                .unwrap()
                .files["image-001.png"],
            b"image bytes"
        );
        assert_eq!(
            source.word_field_codes().unwrap(),
            ["DATE", "PAGE", "TOC"]
                .map(|instruction| json!({"part":"word/document.xml","instruction":instruction}))
        );
        assert_eq!(source.sheets()[0]["cells"][0]["value"], "plain");
    }

    #[test]
    fn word_section_print_settings_are_recorded_with_source_part() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("print.docx");
        let body = format!(
            r#"<w:document xmlns:w="{W}"><w:body><w:p><w:r><w:t>x</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr></w:body></w:document>"#
        );
        word_package(&path, &body, &[]);
        let settings = Source::open(&path).unwrap().print_settings().unwrap();
        assert_eq!(settings.len(), 1);
        assert_eq!(settings[0]["part"], "word/document.xml");
        assert_eq!(settings[0]["component"], "sectPr");
        assert!(settings[0]["xml"].as_str().unwrap().contains("11906"));
    }

    #[test]
    fn word_field_result_edit_is_rejected_before_output_and_plain_run_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fields.docm");
        word_package(
            &source,
            BODY,
            &[("word/vbaProject.bin", b"opaque".to_vec())],
        );
        let Source::Text(doc) = Source::open(&source).unwrap() else {
            panic!("Word source expected")
        };
        // Paragraphs are blocks: the field result and the run after it share A2.
        assert_eq!(
            values(&doc, 0),
            ["plain", "2026/09/25after", "1", "entry", "tail"]
        );
        let change = |cell: &str, before: &str, after: &str| json!({"sheet":"document","cell":cell,"before":before,"after":after});
        for (cell, before, after) in [
            ("A2", "2026/09/25after", "2026/09/26after"),
            ("A3", "1", "2"),
            ("A4", "entry", "edited"),
        ] {
            let rejected = dir.path().join("rejected.docm");
            let error = doc
                .patch(&rejected, &[], &[change(cell, before, after)])
                .unwrap_err();
            assert!(
                error.to_string().contains("Word field result"),
                "{cell}: {error}"
            );
            assert!(!rejected.exists());
        }
        let written = dir.path().join("written.docm");
        doc.patch(
            &written,
            &[],
            &[change("A2", "2026/09/25after", "2026/09/25edited")],
        )
        .unwrap();
        assert_eq!(
            values(&open_word(&written), 0),
            ["plain", "2026/09/25edited", "1", "entry", "tail"]
        );
    }

    #[test]
    fn word_templates_share_the_document_body() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["dotx", "dotm"] {
            let source = dir.path().join(format!("template.{extension}"));
            word_package(&source, BODY, &[]);
            let Source::Text(doc) = Source::open(&source).unwrap() else {
                panic!("Word source expected")
            };
            assert_eq!(doc.sheets()[0]["cells"][0]["value"], "plain");
        }
    }

    #[test]
    fn strict_word_is_rejected_with_resave_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("strict.docx");
        package(
            &source,
            &[
                (
                    "_rels/.rels",
                    format!(
                        r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{STRICT_REL}/officeDocument" Target="word/document.xml"/></Relationships>"#
                    )
                    .into_bytes(),
                ),
                (
                    "word/document.xml",
                    br#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main"/>"#
                        .to_vec(),
                ),
            ],
        );
        let error = Source::open(&source).err().unwrap().to_string();
        assert!(error.contains("Strict Open XML"), "{error}");
    }

    fn compound_file(streams: &[&str]) -> Vec<u8> {
        use std::io::Write;
        let mut file = cfb::CompoundFile::create(std::io::Cursor::new(vec![])).unwrap();
        for name in streams {
            file.create_stream(name)
                .unwrap()
                .write_all(b"data")
                .unwrap();
        }
        file.flush().unwrap();
        file.into_inner().into_inner()
    }

    #[test]
    fn ole_compound_files_are_reported_as_encrypted_or_binary() {
        let password = compound_file(&["/EncryptionInfo", "/EncryptedPackage"]);
        let error = office_package(&password).unwrap_err().to_string();
        assert!(error.contains("--password-stdin"), "{error}");
        let rights = compound_file(&["/EncryptedPackage"]);
        let error = office_package(&rights).unwrap_err().to_string();
        assert!(error.contains("sensitivity label"), "{error}");
        let binary = compound_file(&["/WordDocument"]);
        let error = office_package(&binary).unwrap_err().to_string();
        assert!(error.contains("binary Office document"), "{error}");
        office_package(b"PK").unwrap();
        let dir = tempfile::tempdir().unwrap();
        for extension in ["docx", "xlsm"] {
            let source = dir.path().join(format!("protected.{extension}"));
            fs::write(&source, &password).unwrap();
            let error = Source::open(&source).err().unwrap().to_string();
            assert!(error.contains("encrypted"), "{extension}: {error}");
        }
    }

    #[test]
    fn format_lists_compose_without_duplicates() {
        let inputs = input_formats();
        let unique: std::collections::BTreeSet<_> = inputs.iter().collect();
        assert_eq!(unique.len(), inputs.len());
        assert!(structure_formats().iter().all(|f| inputs.contains(f)));
        assert_eq!(binary_office_replacement("xlsb"), Some(".xlsx/.xlsm"));
        assert_eq!(binary_office_replacement("xlsx"), None);
    }

    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

    fn word_rels(extra: &str) -> (&'static str, Vec<u8>) {
        (
            "word/_rels/document.xml.rels",
            format!(r#"<Relationships xmlns="{PACKAGE_REL}">{extra}</Relationships>"#).into_bytes(),
        )
    }

    fn open_word(source: &Path) -> TextSource {
        let Source::Text(doc) = Source::open(source).unwrap() else {
            panic!("Word source expected")
        };
        doc
    }

    fn values(doc: &TextSource, sheet: usize) -> Vec<String> {
        doc.sheets()[sheet]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["value"].as_str().unwrap().to_owned())
            .collect()
    }

    fn written_part(path: &Path, name: &str) -> Vec<u8> {
        let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        let mut bytes = vec![];
        zip.by_name(name).unwrap().read_to_end(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn text_box_is_read_once_and_edits_reach_both_renderings() {
        let text_box = |text: &str| {
            format!(r#"<w:txbxContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:txbxContent>"#)
        };
        let body = format!(
            r#"<w:document xmlns:w="{W}" xmlns:mc="{MARKUP_COMPATIBILITY}"><w:body><w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing>{}</w:drawing></mc:Choice><mc:Fallback><w:pict>{}</w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>2026/09/25</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>after</w:t></w:r></w:p></w:body></w:document>"#,
            text_box("note"),
            text_box("note")
        );
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("box.docx");
        word_package(&source, &body, &[]);
        let doc = open_word(&source);
        // The anchor paragraph has no text of its own; the text box is its own block.
        assert_eq!(values(&doc, 0), ["note", "2026/09/25after"]);
        let change = |cell: &str, before: &str| json!({"sheet":"document","cell":cell,"before":before,"after":"edited"});
        let error = doc
            .patch(
                &dir.path().join("field.docx"),
                &[],
                &[change("A2", "2026/09/25after")],
            )
            .unwrap_err();
        assert!(error.to_string().contains("Word field result"), "{error}");
        let written = dir.path().join("written.docx");
        doc.patch(&written, &[], &[change("A1", "note")]).unwrap();
        let xml = String::from_utf8(written_part(&written, "word/document.xml")).unwrap();
        assert_eq!(xml.matches(">edited</w:t>").count(), 2, "{xml}");
        assert!(!xml.contains(">note<"));

        let differing = body.replacen(&text_box("note"), &(text_box("a") + &text_box("b")), 2);
        let differing = differing.replacen(&(text_box("a") + &text_box("b")), &text_box("note"), 1);
        let source = dir.path().join("differing.docx");
        word_package(&source, &differing, &[]);
        let doc = open_word(&source);
        assert_eq!(values(&doc, 0)[0], "note");
        let error = doc
            .patch(
                &dir.path().join("differing-out.docx"),
                &[],
                &[change("A1", "note")],
            )
            .unwrap_err();
        assert!(error.to_string().contains("copies"), "{error}");
    }

    #[test]
    fn empty_choice_falls_back_to_the_branch_with_text() {
        let body = format!(
            r#"<w:document xmlns:w="{W}" xmlns:mc="{MARKUP_COMPATIBILITY}"><w:body><w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"/><mc:Fallback><w:t>fallback</w:t></mc:Fallback></mc:AlternateContent></w:r></w:p></w:body></w:document>"#
        );
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fallback.docx");
        word_package(&source, &body, &[]);
        assert_eq!(values(&open_word(&source), 0), ["fallback"]);
    }

    #[test]
    fn percent_encoded_targets_and_comments_are_read() {
        let part = |text: &str, root: &str| {
            format!(r#"<w:{root} xmlns:w="{W}"><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:{root}>"#)
                .into_bytes()
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("encoded.docx");
        word_package(
            &source,
            BODY,
            &[
                word_rels(&format!(
                    r#"<Relationship Id="h1" Type="{REL}/header" Target="header%201.xml"/><Relationship Id="h2" Type="{REL}/header" Target="%E3%83%98%E3%83%83%E3%83%80.xml"/><Relationship Id="h3" Type="{REL}/footer" Target="footer%201.xml"/><Relationship Id="c" Type="{REL}/comments" Target="comments.xml"/>"#
                )),
                ("word/header 1.xml", part("spaced", "hdr")),
                ("word/ヘッダ.xml", part("japanese", "hdr")),
                ("word/footer%201.xml", part("literal", "ftr")),
                ("word/comments.xml", part("comment body", "comments")),
            ],
        );
        let doc = open_word(&source);
        let names: Vec<_> = doc
            .sheets()
            .iter()
            .map(|s| s["part"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "word/document.xml",
                "word/header 1.xml",
                "word/ヘッダ.xml",
                "word/footer%201.xml",
                "word/comments.xml"
            ]
        );
        assert_eq!(values(&doc, 4), ["comment body"]);
        assert!(
            doc.sheets()[4]["name"]
                .as_str()
                .unwrap()
                .starts_with("comments-")
        );

        for target in ["..%2Fescape.xml", "bad%zz.xml"] {
            let source = dir.path().join("rejected.docx");
            let _ = fs::remove_file(&source);
            word_package(
                &source,
                BODY,
                &[word_rels(&format!(
                    r#"<Relationship Id="h1" Type="{REL}/header" Target="{target}"/>"#
                ))],
            );
            assert!(Source::open(&source).is_err(), "{target}");
        }
    }

    #[test]
    fn utf16_parts_are_read_and_written_back_in_utf16() {
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-16" standalone="yes"?><w:document xmlns:w="{W}"><w:body><w:p><w:r><w:t>本文</w:t></w:r></w:p></w:body></w:document>"#
        );
        let encoded = |text: &str| -> Vec<u8> {
            [0xFF, 0xFE]
                .into_iter()
                .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
                .collect()
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("utf16.docx");
        package(
            &source,
            &[
                (
                    "_rels/.rels",
                    format!(
                        r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/officeDocument" Target="word/document.xml"/></Relationships>"#
                    )
                    .into_bytes(),
                ),
                ("word/document.xml", encoded(&body)),
            ],
        );
        let doc = open_word(&source);
        assert_eq!(values(&doc, 0), ["本文"]);
        let written = dir.path().join("written.docx");
        doc.patch(
            &written,
            &[],
            &[json!({"sheet":"document","cell":"A1","before":"本文","after":"更新"})],
        )
        .unwrap();
        assert_eq!(
            written_part(&written, "word/document.xml"),
            encoded(&body.replace("<w:t>本文", r#"<w:t xml:space="preserve">更新"#))
        );
        assert_eq!(values(&open_word(&written), 0), ["更新"]);
    }

    #[test]
    fn word_line_breaks_and_tabs_are_written_as_run_elements() {
        let text_box = || r#"<w:txbxContent><w:p><w:r><w:t>注</w:t><w:br/><w:t>記</w:t></w:r></w:p></w:txbxContent>"#;
        let body = format!(
            r#"<w:document xmlns:w="{W}" xmlns:mc="{MARKUP_COMPATIBILITY}"><w:body><w:p><w:r><w:t>項目</w:t><w:tab/><w:t>値</w:t></w:r></w:p><w:p><w:r><w:t>一行目</w:t><w:br/><w:t>二行目</w:t></w:r></w:p><w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing>{}</w:drawing></mc:Choice><mc:Fallback><w:pict>{}</w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p><w:p><w:fldSimple w:instr="PAGE"><w:r><w:t>1</w:t><w:tab/><w:t>2</w:t></w:r></w:fldSimple></w:p></w:body></w:document>"#,
            text_box(),
            text_box()
        );
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("breaks.docx");
        word_package(&source, &body, &[]);
        let doc = open_word(&source);
        assert_eq!(
            values(&doc, 0),
            ["項目\t値", "一行目\n二行目", "注\n記", "1\t2"]
        );
        let change = |cell: &str, before: &str, after: &str| json!({"sheet":"document","cell":cell,"before":before,"after":after});
        // A tab or break in a field result is recalculated like its text.
        let error = doc
            .patch(
                &dir.path().join("field.docx"),
                &[],
                &[change("A4", "1\t2", "12")],
            )
            .unwrap_err();
        assert!(error.to_string().contains("Word field result"), "{error}");
        let written = dir.path().join("written.docx");
        doc.patch(
            &written,
            &[],
            &[
                change("A1", "項目\t値", "項目\n値\t（単位）"),
                change("A2", "一行目\n二行目", "一行目二行目"),
                change("A3", "注\n記", "注\t記"),
            ],
        )
        .unwrap();
        assert_eq!(
            values(&open_word(&written), 0),
            ["項目\n値\t（単位）", "一行目二行目", "注\t記", "1\t2"]
        );
        let xml = String::from_utf8(written_part(&written, "word/document.xml")).unwrap();
        assert!(
            xml.contains(r#"<w:t>項目</w:t><w:br/><w:t xml:space="preserve">値</w:t><w:tab/><w:t xml:space="preserve">（単位）</w:t>"#),
            "{xml}"
        );
        assert!(xml.contains("<w:t>一行目</w:t><w:t>二行目</w:t>"), "{xml}");
        // Both renderings of the text box change.
        assert_eq!(
            xml.matches("<w:t>注</w:t><w:tab/><w:t>記</w:t>").count(),
            2,
            "{xml}"
        );
    }

    /// A Word package whose title shows the core properties' title twice (a
    /// block control and an inline one split over two runs) and whose cover
    /// page date and abstract show a custom XML part.
    fn bound_package(path: &Path) {
        const DC: &str = "http://purl.org/dc/elements/1.1/";
        const CP: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
        const COVER: &str = "http://schemas.microsoft.com/office/2006/coverPageProps";
        let binding = |store: &str, prefixes: &str, xpath: &str, kind: &str| {
            format!(
                r#"<w:sdtPr><w:dataBinding w:prefixMappings="{prefixes}" w:xpath="{xpath}" w:storeItemID="{store}"/>{kind}</w:sdtPr>"#
            )
        };
        let title = binding(
            "{6C3C8BC8-F283-45AE-878A-BAB7291924A1}",
            &format!("xmlns:ns0='{DC}' xmlns:ns1='{CP}'"),
            "/ns1:coreProperties[1]/ns0:title[1]",
            "<w:text/>",
        );
        let cover = |field: &str, kind: &str| {
            binding(
                "{55AF091B-3C7A-41E3-B477-F2FDAA23CFDA}",
                &format!("xmlns:ns0='{COVER}'"),
                &format!("/ns0:CoverPageProperties[1]/ns0:{field}[1]"),
                kind,
            )
        };
        let body = format!(
            r#"<w:document xmlns:w="{W}"><w:body><w:sdt>{title}<w:sdtContent><w:p><w:r><w:t>旧題</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>題名：</w:t></w:r><w:sdt>{title}<w:sdtContent><w:r><w:t>旧</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>題</w:t></w:r></w:sdtContent></w:sdt></w:p><w:sdt>{}<w:sdtContent><w:p><w:r><w:t>2026/9/1</w:t></w:r></w:p></w:sdtContent></w:sdt><w:sdt>{}<w:sdtContent><w:p><w:r><w:t>概要</w:t></w:r></w:p></w:sdtContent></w:sdt></w:body></w:document>"#,
            cover("PublishDate", "<w:date/>"),
            cover("Abstract", "<w:text/>")
        );
        package(
            path,
            &[
                (
                    "_rels/.rels",
                    format!(
                        r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/officeDocument" Target="word/document.xml"/><Relationship Id="r2" Type="{PACKAGE_REL}/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#
                    )
                    .into_bytes(),
                ),
                ("word/document.xml", body.into_bytes()),
                (
                    "docProps/core.xml",
                    format!(r#"<cp:coreProperties xmlns:cp="{CP}" xmlns:dc="{DC}"><dc:title>旧題</dc:title><dc:creator/></cp:coreProperties>"#).into_bytes(),
                ),
                (
                    "customXml/item1.xml",
                    format!(r#"<CoverPageProperties xmlns="{COVER}"><PublishDate>2026-09-01</PublishDate><Abstract>概要</Abstract></CoverPageProperties>"#).into_bytes(),
                ),
                (
                    "customXml/_rels/item1.xml.rels",
                    format!(r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="p" Type="{REL}/customXmlProps" Target="itemProps1.xml"/></Relationships>"#).into_bytes(),
                ),
                (
                    "customXml/itemProps1.xml",
                    br#"<ds:datastoreItem ds:itemID="{55af091b-3c7a-41e3-b477-f2fdaa23cfda}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"/>"#.to_vec(),
                ),
            ],
        );
    }

    #[test]
    fn edits_to_bound_content_controls_update_their_data_and_twins() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bound.docx");
        bound_package(&source);
        let doc = open_word(&source);
        assert_eq!(values(&doc, 0), ["旧題", "題名：旧題", "2026/9/1", "概要"]);
        let change = |cell: &str, before: &str, after: &str| json!({"sheet":"document","cell":cell,"before":before,"after":after});
        let written = dir.path().join("written.docx");
        doc.patch(
            &written,
            &[],
            &[
                change("A1", "旧題", "新題"),
                change("A4", "概要", "新しい概要"),
            ],
        )
        .unwrap();
        // The inline control bound to the same title shows it too.
        assert_eq!(
            values(&open_word(&written), 0),
            ["新題", "題名：新題", "2026/9/1", "新しい概要"]
        );
        let core = String::from_utf8(written_part(&written, "docProps/core.xml")).unwrap();
        assert!(
            core.contains("<dc:title>新題</dc:title><dc:creator/>"),
            "{core}"
        );
        let cover = String::from_utf8(written_part(&written, "customXml/item1.xml")).unwrap();
        assert!(cover.contains("<Abstract>新しい概要</Abstract>"), "{cover}");

        for (changes, message) in [
            (vec![change("A3", "2026/9/1", "2026/10/1")], "date, list"),
            (vec![change("A1", "旧題", "新\n題")], "single-line"),
            (
                vec![
                    change("A1", "旧題", "新題"),
                    change("A2", "題名：旧題", "題名：別題"),
                ],
                "edited to different text",
            ),
        ] {
            let error = doc
                .patch(&dir.path().join("rejected.docx"), &[], &changes)
                .unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    /// A one-slide presentation whose slide holds `slide` text and, with
    /// `notes`, a notes page with that text, its slide image and slide number.
    fn pptx_package(path: &Path, slide: &str, notes: Option<&str>) {
        let mut parts = vec![
            (
                "_rels/.rels",
                format!(
                    r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/presentation.xml",
                format!(
                    r#"<p:presentation xmlns:p="{PRESENTATION}" xmlns:r="{REL}"><p:sldIdLst><p:sldId id="256" r:id="s1"/></p:sldIdLst></p:presentation>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                format!(
                    r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="s1" Type="{REL}/slide" Target="slides/slide1.xml"/></Relationships>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(
                    "<p:sld xmlns:p=\"{PRESENTATION}\" xmlns:a=\"{DRAWING}\"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:rPr lang=\"ja-JP\" b=\"1\"/><a:t>{slide}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
                )
                .into_bytes(),
            ),
        ];
        if let Some(notes) = notes {
            let placeholder = |kind: &str, body: &str| {
                format!(
                    r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="{kind}"/><p:cNvSpPr/><p:nvPr><p:ph type="{kind}"/></p:nvPr></p:nvSpPr>{body}</p:sp>"#
                )
            };
            parts.push((
                "ppt/slides/_rels/slide1.xml.rels",
                format!(
                    r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="n1" Type="{REL}/notesSlide" Target="../notesSlides/notesSlide1.xml"/></Relationships>"#
                )
                .into_bytes(),
            ));
            parts.push((
                "ppt/notesSlides/notesSlide1.xml",
                format!(
                    r#"<p:notes xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}"><p:cSld><p:spTree>{}{}{}</p:spTree></p:cSld></p:notes>"#,
                    placeholder("sldImg", ""),
                    placeholder("body", &format!("<p:txBody><a:p><a:r><a:t>{notes}</a:t></a:r></a:p></p:txBody>")),
                    placeholder("sldNum", r#"<p:txBody><a:p><a:fld id="{1}" type="slidenum"><a:t>1</a:t></a:fld></a:p></p:txBody>"#),
                )
                .into_bytes(),
            ));
        }
        package(path, &parts);
    }

    fn open_text(path: &Path) -> TextSource {
        let Source::Text(doc) = Source::open(path).unwrap() else {
            panic!("text source expected")
        };
        doc
    }

    #[test]
    fn powerpoint_paragraphs_take_tabs_and_line_breaks_in_the_run_format() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("tabs.pptx");
        pptx_package(&source, "項目\t値", None);
        let doc = open_text(&source);
        assert_eq!(values(&doc, 0), ["項目\t値"]);
        let written = dir.path().join("written.pptx");
        doc.patch(
            &written,
            &[],
            &[json!({"sheet":"slide-1","cell":"A1","before":"項目\t値","after":"項目\n\t値"})],
        )
        .unwrap();
        let doc = open_text(&written);
        assert_eq!(values(&doc, 0), ["項目\n\t値"]);
        let slide = String::from_utf8(written_part(&written, "ppt/slides/slide1.xml")).unwrap();
        let format = r#"<a:rPr lang="ja-JP" b="1"/>"#;
        assert!(
            slide.contains(&format!(
                "<a:r>{format}<a:t>項目</a:t></a:r><a:br>{format}</a:br><a:r>{format}<a:t>\t値</a:t></a:r>"
            )),
            "{slide}"
        );
        // Taking the line break out joins the lines again.
        let joined = dir.path().join("joined.pptx");
        doc.patch(
            &joined,
            &[],
            &[json!({"sheet":"slide-1","cell":"A1","before":"項目\n\t値","after":"項目、値"})],
        )
        .unwrap();
        assert_eq!(values(&open_text(&joined), 0), ["項目、値"]);
    }

    /// Notes pages follow the slides; the slide number they show is left out.
    #[test]
    fn powerpoint_notes_follow_the_slides_and_are_written_back() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("notes.pptx");
        pptx_package(&source, "表題", Some("話す内容"));
        let doc = open_text(&source);
        let names: Vec<_> = doc
            .sheets()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["slide-1", "notes-1"]);
        assert_eq!(doc.sheets()[1]["part"], "ppt/notesSlides/notesSlide1.xml");
        assert_eq!(values(&doc, 1), ["話す内容"]);
        let written = dir.path().join("written.pptx");
        doc.patch(
            &written,
            &[], &[json!({"sheet":"notes-1","cell":"A1","before":"話す内容","after":"話す内容（改訂）"})],
        )
        .unwrap();
        let doc = open_text(&written);
        assert_eq!(values(&doc, 0), ["表題"]);
        assert_eq!(values(&doc, 1), ["話す内容（改訂）"]);
        let notes =
            String::from_utf8(written_part(&written, "ppt/notesSlides/notesSlide1.xml")).unwrap();
        assert!(notes.contains("<a:t>1</a:t></a:fld>"), "{notes}");
    }

    /// A two-slide presentation. Slide 1 has a notes page, a chart with its
    /// workbook and a comment; with `link`, slide 2 links to slide 1. Both
    /// slides are in one section, and custom shows list `shows` (slide numbers).
    fn deck_package(path: &Path, link: bool, shows: &[&[usize]]) {
        const P14: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";
        const TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
        let slide = |text: &str| {
            format!(
                r#"<p:sld xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}" xmlns:r="{REL}"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
            )
            .into_bytes()
        };
        let rels = |body: &str| {
            format!(r#"<Relationships xmlns="{PACKAGE_REL}">{body}</Relationships>"#).into_bytes()
        };
        let override_type = |part: &str, kind: &str| {
            format!(
                r#"<Override PartName="/{part}" ContentType="application/vnd.openxmlformats-officedocument.{kind}+xml"/>"#
            )
        };
        let shows: String = shows
            .iter()
            .enumerate()
            .map(|(i, slides)| {
                let slides: String = slides
                    .iter()
                    .map(|n| format!(r#"<p:sld r:id="rId{}"/>"#, n + 1))
                    .collect();
                format!(r#"<p:custShow name="show{i}" id="{i}"><p:sldLst>{slides}</p:sldLst></p:custShow>"#)
            })
            .collect();
        let parts = vec![
            (
                "[Content_Types].xml",
                format!(
                    r#"<Types xmlns="{TYPES}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="xlsx" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"/>{}{}{}{}{}</Types>"#,
                    override_type("ppt/presentation.xml", "presentationml.presentation.main"),
                    override_type("ppt/slides/slide1.xml", "presentationml.slide"),
                    override_type("ppt/slides/slide2.xml", "presentationml.slide"),
                    override_type("ppt/notesSlides/notesSlide1.xml", "presentationml.notesSlide"),
                    override_type("ppt/charts/chart1.xml", "drawingml.chart"),
                )
                .into_bytes(),
            ),
            (
                "_rels/.rels",
                rels(&format!(
                    r#"<Relationship Id="r1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="r2" Type="{REL}/extended-properties" Target="docProps/app.xml"/>"#
                )),
            ),
            (
                "docProps/app.xml",
                b"<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Slides>2</Slides><Notes>1</Notes></Properties>".to_vec(),
            ),
            (
                "ppt/presentation.xml",
                format!(
                    r#"<p:presentation xmlns:p="{PRESENTATION}" xmlns:r="{REL}"><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst><p:custShowLst>{shows}</p:custShowLst><p:extLst><p:ext uri="{{521415D9-36F7-43E2-AB2F-B90AF26B5E84}}"><p14:sectionLst xmlns:p14="{P14}"><p14:section name="All" id="{{1}}"><p14:sldIdLst><p14:sldId id="256"/><p14:sldId id="257"/></p14:sldIdLst></p14:section></p14:sectionLst></p:ext></p:extLst></p:presentation>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                rels(&format!(
                    r#"<Relationship Id="rId2" Type="{REL}/slide" Target="slides/slide1.xml"/><Relationship Id="rId3" Type="{REL}/slide" Target="slides/slide2.xml"/>"#
                )),
            ),
            ("ppt/slides/slide1.xml", slide("表題")),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                rels(&format!(
                    r#"<Relationship Id="rId1" Type="{REL}/notesSlide" Target="../notesSlides/notesSlide1.xml"/><Relationship Id="rId2" Type="{REL}/chart" Target="../charts/chart1.xml"/><Relationship Id="rId3" Type="{REL}/comments" Target="../comments/comment1.xml"/>"#
                )),
            ),
            (
                "ppt/notesSlides/notesSlide1.xml",
                format!(
                    r#"<p:notes xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="body"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>話す内容</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/notesSlides/_rels/notesSlide1.xml.rels",
                rels(&format!(
                    r#"<Relationship Id="rId1" Type="{REL}/slide" Target="../slides/slide1.xml"/>"#
                )),
            ),
            ("ppt/charts/chart1.xml", b"<chartSpace/>".to_vec()),
            (
                "ppt/charts/_rels/chart1.xml.rels",
                rels(&format!(
                    r#"<Relationship Id="rId1" Type="{REL}/package" Target="../embeddings/Book.xlsx"/>"#
                )),
            ),
            ("ppt/embeddings/Book.xlsx", b"workbook".to_vec()),
            ("ppt/comments/comment1.xml", b"<cmLst/>".to_vec()),
            ("ppt/slides/slide2.xml", slide("二枚目")),
            (
                "ppt/slides/_rels/slide2.xml.rels",
                rels(&if link {
                    format!(r#"<Relationship Id="rId1" Type="{REL}/slide" Target="slide1.xml"/>"#)
                } else {
                    String::new()
                }),
            ),
        ];
        package(path, &parts);
    }

    fn slide_operation(kind: &str, fields: Value) -> Value {
        let mut operation = json!({"id":"op","kind":kind,"reason":"test"});
        for (key, value) in fields.as_object().unwrap() {
            operation[key] = value.clone();
        }
        operation
    }

    #[test]
    fn powerpoint_slides_are_copied_with_their_own_parts_and_deleted_with_theirs() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("deck.pptx");
        deck_package(&source, false, &[&[1, 2]]);
        let doc = open_text(&source);
        let names: Vec<_> = doc
            .sheets()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["slide-1", "slide-2", "notes-1"]);
        let operations = [
            json!({"id":"copy","kind":"insert_slide","from":"slide-1","after":"slide-2","reason":"copy"}),
            json!({"id":"drop","kind":"delete_slide","slide":"slide-2","reason":"drop"}),
        ];
        let written = dir.path().join("written.pptx");
        let report = doc
            .patch(
                &written,
                &operations,
                &[
                    json!({"sheet":"copy","cell":"A1","before":"表題","after":"複製"}),
                    json!({"sheet":"notes-copy","cell":"A1","before":"話す内容","after":"複製のノート"}),
                ],
            )
            .unwrap();
        assert_eq!(report["slide_order"], json!(["slide-1", "copy"]));
        let result = open_text(&written);
        let pages: Vec<_> = (0..result.sheets().len())
            .map(|i| values(&result, i))
            .collect();
        assert_eq!(pages, [["表題"], ["複製"], ["話す内容"], ["複製のノート"]]);

        let part = |name: &str| String::from_utf8(written_part(&written, name)).unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(&written).unwrap()).unwrap();
        assert!(zip.by_name("ppt/slides/slide2.xml").is_err());
        assert!(zip.by_name("ppt/slides/_rels/slide2.xml.rels").is_err());
        // The copy has its own chart and workbook, and no comments.
        let copy_rels = part("ppt/slides/_rels/slide3.xml.rels");
        assert!(
            copy_rels.contains(r#"Target="../charts/chart2.xml""#),
            "{copy_rels}"
        );
        assert!(
            copy_rels.contains(r#"Target="../notesSlides/notesSlide2.xml""#),
            "{copy_rels}"
        );
        assert!(!copy_rels.contains("comments"), "{copy_rels}");
        assert!(
            part("ppt/charts/_rels/chart2.xml.rels")
                .contains(r#"Target="../embeddings/Book1.xlsx""#)
        );
        assert_eq!(
            written_part(&written, "ppt/embeddings/Book1.xlsx"),
            b"workbook"
        );
        assert!(
            part("ppt/notesSlides/_rels/notesSlide2.xml.rels")
                .contains(r#"Target="../slides/slide3.xml""#)
        );
        assert!(part("ppt/slides/_rels/slide1.xml.rels").contains("comment1.xml"));

        let presentation = part("ppt/presentation.xml");
        assert!(presentation.contains(r#"<p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="258" r:id="rId1"/></p:sldIdLst>"#), "{presentation}");
        assert!(
            presentation.contains(r#"<p:sldLst><p:sld r:id="rId2"/></p:sldLst>"#),
            "{presentation}"
        );
        assert!(
            presentation.contains(
                r#"<p14:sldIdLst><p14:sldId id="256"/><p14:sldId id="258"/></p14:sldIdLst>"#
            ),
            "{presentation}"
        );
        let types = part("[Content_Types].xml");
        for present in [
            "/ppt/slides/slide3.xml",
            "/ppt/notesSlides/notesSlide2.xml",
            "/ppt/charts/chart2.xml",
        ] {
            assert!(types.contains(present), "{present}: {types}");
        }
        assert!(!types.contains("/ppt/slides/slide2.xml"), "{types}");
        assert!(part("docProps/app.xml").contains("<Slides>2</Slides><Notes>2</Notes>"));
    }

    #[test]
    fn a_linked_slide_or_the_last_of_a_custom_show_is_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let delete = |source: &Path, slide: &str| {
            open_text(source)
                .patch(
                    &dir.path().join(format!("out-{slide}.pptx")),
                    &[slide_operation("delete_slide", json!({"slide":slide}))],
                    &[],
                )
                .map(|_| String::new())
                .unwrap_or_else(|error| format!("{error:#}"))
        };
        let linked = dir.path().join("linked.pptx");
        deck_package(&linked, true, &[]);
        let error = delete(&linked, "slide-1");
        assert!(error.contains("linked from slide-2"), "{error}");
        let shown = dir.path().join("shown.pptx");
        deck_package(&shown, false, &[&[2]]);
        let error = delete(&shown, "slide-2");
        assert!(
            error.contains("custom show show0 would have no slides left"),
            "{error}"
        );
        assert!(!dir.path().join("out-slide-2.pptx").exists());
    }

    #[test]
    fn powerpoint_slides_are_moved_with_their_sections_and_hidden_or_shown() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("deck.pptx");
        deck_package(&source, false, &[&[1, 2]]);
        let doc = open_text(&source);
        let operations = [
            json!({"id":"copy","kind":"insert_slide","from":"slide-1","after":"slide-2","reason":"copy"}),
            json!({"id":"first","kind":"move_slide","slide":"slide-2","before":"slide-1","reason":"order"}),
            json!({"id":"last","kind":"move_slide","slide":"slide-1","after":"copy","reason":"order"}),
            json!({"id":"hide","kind":"set_slide_visibility","slide":"copy","hidden":true,"reason":"draft"}),
        ];
        let written = dir.path().join("written.pptx");
        let report = doc.patch(&written, &operations, &[]).unwrap();
        assert_eq!(report["slide_order"], json!(["slide-2", "copy", "slide-1"]));
        let result = open_text(&written);
        assert_eq!(result.sheets().len(), 5, "three slides and two notes pages");
        let states: Vec<_> = result.sheets()[..3]
            .iter()
            .map(|s| s["state"].clone())
            .collect();
        assert_eq!(
            states,
            [json!("visible"), json!("hidden"), json!("visible")]
        );
        let texts: Vec<_> = (0..3).map(|i| values(&result, i)).collect();
        assert_eq!(texts, [vec!["二枚目"], vec!["表題"], vec!["表題"]]);
        // The section lists the slides in their new order, as the show does.
        let mut zip = zip::ZipArchive::new(fs::File::open(&written).unwrap()).unwrap();
        let mut presentation = String::new();
        zip.by_name("ppt/presentation.xml")
            .unwrap()
            .read_to_string(&mut presentation)
            .unwrap();
        let ids = |list: &str| -> Vec<String> {
            let start = presentation.find(list).unwrap();
            let end = start
                + presentation[start..]
                    .find(&list.replace('<', "</"))
                    .unwrap();
            regex::Regex::new(r#"id="(\d+)""#)
                .unwrap()
                .captures_iter(&presentation[start..end])
                .map(|c| c[1].to_owned())
                .collect()
        };
        assert_eq!(ids("<p:sldIdLst>"), ids("<p14:sldIdLst>"));
        assert_eq!(ids("<p:sldIdLst>")[0], "257");
        // Showing the hidden slide again removes the mark.
        let shown = dir.path().join("shown.pptx");
        let hidden = open_text(&written);
        hidden
            .patch(
                &shown,
                &[json!({"id":"show","kind":"set_slide_visibility","slide":"slide-2","hidden":false,"reason":"final"})],
                &[],
            )
            .unwrap();
        assert!(
            open_text(&shown)
                .sheets()
                .iter()
                .all(|s| s["state"] == "visible")
        );
        let error = |operations: &[Value]| {
            format!(
                "{:#}",
                parse_slide_operations(operations, doc.sheets()).unwrap_err()
            )
        };
        assert!(
            error(&[json!({"id":"m","kind":"move_slide","slide":"slide-1","after":"slide-1","reason":"r"})])
                .contains("next to itself")
        );
        assert!(
            error(&[json!({"id":"m","kind":"move_slide","slide":"slide-9","after":"slide-1","reason":"r"})])
                .contains("slide-9 is not a slide here")
        );
        assert!(
            error(&[json!({"id":"m","kind":"move_slide","slide":"slide-1","reason":"r"})])
                .contains("one of after and before")
        );
    }

    #[test]
    fn slide_operations_name_existing_slides_and_new_pages() {
        let sheets = [
            json!({"name":"slide-1","part":"a"}),
            json!({"name":"slide-2","part":"b"}),
            json!({"name":"notes-1","part":"c"}),
        ];
        let error = |operations: &[Value]| {
            format!(
                "{:#}",
                parse_slide_operations(operations, &sheets).unwrap_err()
            )
        };
        let insert = |id: &str, from: &str| json!({"id":id,"kind":"insert_slide","from":from,"after":"slide-1","reason":"r"});
        assert!(error(&[insert("slide-2", "slide-1")]).contains("choose another ID"));
        assert!(error(&[insert("1", "slide-1")]).contains("is a number"));
        assert!(error(&[insert("x", "slide-9")]).contains("slide-9 is not a slide here"));
        let delete_copy = json!({"id":"d","kind":"delete_slide","slide":"x","reason":"r"});
        assert!(error(&[insert("x", "slide-1"), delete_copy]).contains("remove that operation"));
        let delete = |id: &str, slide: &str| json!({"id":id,"kind":"delete_slide","slide":slide,"reason":"r"});
        assert!(error(&[delete("a", "slide-1"), delete("b", "slide-2")]).contains("last slide"));
        assert!(
            error(&[delete("a", "slide-1"), delete("b", "slide-1")]).contains("not a slide here")
        );

        let operations =
            parse_slide_operations(&[insert("x", "slide-1"), delete("d", "slide-1")], &sheets)
                .unwrap();
        let view = slide_view(&sheets, &[], &operations).unwrap();
        let pages: Vec<_> = view
            .iter()
            .map(|s| (s["name"].as_str().unwrap(), s["page"].as_str().unwrap()))
            .collect();
        assert_eq!(
            pages,
            [
                ("slide-2", "sheet-2"),
                ("x", "sheet-x"),
                ("notes-x", "sheet-notes-x")
            ]
        );
        assert_eq!(view[2]["copy_of"], "notes-1");
    }

    #[test]
    fn root_level_presentation_relationships_are_found() {
        let mut parts = BTreeMap::new();
        parts.insert(
            "_rels/.rels".into(),
            format!("<Relationships xmlns=\"{PACKAGE_REL}\"><Relationship Id=\"r0\" Type=\"{REL}/officeDocument\" Target=\"presentation.xml\"/></Relationships>").into_bytes(),
        );
        parts.insert(
            "presentation.xml".into(),
            format!("<p:presentation xmlns:p=\"{PRESENTATION}\" xmlns:r=\"{REL}\"><p:sldId r:id=\"r1\"/></p:presentation>").into_bytes(),
        );
        parts.insert(
            "_rels/presentation.xml.rels".into(),
            format!("<Relationships xmlns=\"{PACKAGE_REL}\"><Relationship Id=\"r1\" Type=\"{REL}/slide\" Target=\"slide.xml\"/></Relationships>").into_bytes(),
        );
        parts.insert("slide.xml".into(), Vec::new());
        parts.insert(
            "_rels/slide.xml.rels".into(),
            format!("<Relationships xmlns=\"{PACKAGE_REL}\"><Relationship Id=\"n1\" Type=\"{REL}/notesSlide\" Target=\"notes.xml\"/></Relationships>").into_bytes(),
        );
        parts.insert("notes.xml".into(), Vec::new());
        assert_eq!(
            office_containers(&parts, "pptx").unwrap(),
            vec![
                ("slide-1".into(), "slide.xml".into()),
                ("notes-1".into(), "notes.xml".into())
            ]
        );
    }

    #[test]
    fn a_path_painted_with_a_gradient_has_the_colour_the_gradient_starts_from() {
        use lopdf::dictionary;
        let mut doc = Pdf::with_version("1.5");
        let step = |from: [f32; 3], to: [f32; 3]| {
            dictionary! { "FunctionType" => 2, "N" => 1,
            "C0" => from.map(Object::Real).to_vec(), "C1" => to.map(Object::Real).to_vec() }
        };
        let stitched = doc.add_object(dictionary! { "FunctionType" => 3,
        "Functions" => vec![step([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]).into(), step([0.0, 0.0, 1.0], [1.0, 1.0, 1.0]).into()] });
        let shaded = |function: Object| {
            dictionary! { "PatternType" => 2,
            "Shading" => dictionary! { "ShadingType" => 2, "ColorSpace" => "DeviceRGB", "Function" => function } }
        };
        // Samples of 8 bits: the first of them is the colour at the start.
        let sampled = doc.add_object(Stream::new(
            dictionary! { "FunctionType" => 0, "Size" => vec![2.into()], "BitsPerSample" => 8,
            "Domain" => vec![0.into(), 1.into()],
            "Range" => vec![0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into()] },
            vec![0, 255, 0, 255, 255, 255],
        ));
        let patterns = dictionary! {
            "P1" => shaded(stitched.into()),
            "P2" => shaded(sampled.into()),
            "P3" => dictionary! { "PatternType" => 1 },
        };
        let content = b"/Pattern cs /P1 scn 0 0 10 10 re f /P2 scn 20 0 10 10 re f \
/P3 scn 40 0 10 10 re f /Pattern CS /P1 SCN 60 0 m 70 0 l S"
            .to_vec();
        let content = doc.add_object(Stream::new(dictionary! {}, content));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            "Resources" => dictionary! { "Pattern" => patterns },
        });
        doc.objects.insert(
            pages,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        let raw = doc.get_page_content_with_limit(page, MAX_PAGE).unwrap();
        let (content, _) = pdf_content(&raw).unwrap();
        let paths = pdf_paths(&content, [0.0, 100.0], &pdf_pattern_colors(&doc, page));
        let colors: Vec<_> = paths
            .iter()
            .map(|p| (p.fill.as_deref(), p.stroke.as_deref()))
            .collect();
        // A tiling pattern has no colour: its path is left unpainted.
        assert_eq!(
            colors,
            [
                (Some("FF0000"), None),
                (Some("00FF00"), None),
                (None, None),
                (None, Some("FF0000"))
            ]
        );
    }
}
