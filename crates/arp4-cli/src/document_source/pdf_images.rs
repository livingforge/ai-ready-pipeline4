//! The images a PDF's pages draw, made into files that an image viewer and
//! Windows OCR open. They are imported as assets like the pictures of Word
//! and PowerPoint, so that a page scanned or saved from an image is read too.
//!
//! JPEG is kept as it is unless its Decode array inverts it; samples, CCITT
//! fax, JBIG2 and JPEG 2000 images are decoded and written as 8-bit gray or
//! RGB PNG, which a browser shows too, with the transparency the image's soft
//! mask, mask or JPEG 2000 alpha gives it. The images of the pages'
//! annotations (stamps, signatures, pictures pasted on a page) are found with
//! those of their content, as are those of the tiling patterns it paints and
//! the soft masks it sets. A picture written in strips or tiles, and the
//! glyph images of a page's Type3 fonts (a bitmap font, text kept as
//! images), are each made one image, as OCR reads them together.

use super::ImageAssets;
use anyhow::{Context, Result, bail, ensure};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, content::Content};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

/// The most bytes an image's samples may decode to.
const MAX_SAMPLES: usize = 512 * 1024 * 1024;
/// The most pixels an image written as PNG may have.
const MAX_PIXELS: u64 = 100_000_000;
/// How deep Form XObjects may draw one another.
const MAX_FORM_DEPTH: usize = 16;
/// The fewest pixels, and pixels along its shorter side, an inline image
/// needs to be taken: smaller ones are bullets, rules and other marks that
/// hold no text to read.
const MIN_INLINE_PIXELS: i64 = 1024;
const MIN_INLINE_SIDE: i64 = 8;

/// An image a page draws: an image XObject, with the color space its
/// ColorSpace names in the resources of what draws it when it names one
/// there; an inline image with its dictionary written out in full; or the
/// images drawn as the parts of one picture, made one image.
pub(super) enum Drawn {
    Object(ObjectId, Option<Box<Object>>),
    Inline(Box<Stream>),
    Composite(Vec<Piece>),
}

/// An image drawn as a part of a picture (a strip of a picture written in
/// strips, a glyph of a Type3 font drawn as an image), with the matrix that
/// places it on the page and whether it is a stencil mask.
pub(super) struct Piece {
    image: Drawn,
    matrix: [f64; 6],
    mask: bool,
}

/// An image a page draws: its index among the images of the document, in
/// the order the pages first draw them, the matrix that maps its unit square
/// onto the page (in the page's user space), and whether it is a stencil
/// mask, which paints only where its samples say. An image that does not
/// show where it is drawn, the tile of a pattern or the shape of a soft
/// mask, has no matrix.
pub struct Placement {
    pub index: usize,
    pub matrix: Option<[f64; 6]>,
    pub mask: bool,
}

/// An image found as a page is walked: where it is drawn, whether it is a
/// stencil mask, whether it shows there, and whether it is drawn by a glyph
/// of a Type3 font.
struct Found {
    image: Drawn,
    matrix: [f64; 6],
    mask: bool,
    shown: bool,
    glyph: bool,
}

/// The images of a PDF by their index, and the images each page (by its
/// number) draws.
pub(super) struct Placements {
    pub images: Vec<Drawn>,
    pub pages: Vec<(u32, Vec<Placement>)>,
}

/// The images the pages of `doc` draw, page by page, and each image by its
/// index: those their content draws, directly, in the Form XObjects, tiling
/// patterns and Type3 glyphs it draws and in the soft masks it sets, and
/// those of their annotations. An image drawn more than once, and inline
/// images alike in data and dictionary, have one index; a picture drawn in
/// strips, and the glyph images of a page, are one image of their own.
/// Inline images too small to hold text are left out.
pub(super) fn placements(doc: &Pdf, max_page: usize) -> Result<Placements> {
    let mut order = vec![];
    let mut objects: BTreeMap<ObjectId, usize> = BTreeMap::new();
    let mut inline: BTreeMap<(String, Vec<u8>), usize> = BTreeMap::new();
    let mut pages = vec![];
    for (page, id) in doc.get_pages() {
        let raw = doc.get_page_content_with_limit(id, max_page)?;
        let content = operations(&raw)?;
        let resources = super::pdf_page_resources(doc, id);
        let mut walker = Walker::new(doc);
        walker.content(&content, &raw, &resources, IDENTITY, 0);
        for (appearance, onto, _) in appearances(doc, id) {
            walker.form(appearance, &resources, onto, 0);
        }
        let placed = gathered(doc, walker.found)
            .into_iter()
            .map(|found| {
                let next = order.len();
                let index = match &found.image {
                    Drawn::Object(id, _) => *objects.entry(*id).or_insert(next),
                    Drawn::Inline(stream) => *inline
                        .entry((format!("{:?}", stream.dict), stream.content.clone()))
                        .or_insert(next),
                    Drawn::Composite(_) => next,
                };
                if index == next {
                    order.push(found.image);
                }
                Placement {
                    index,
                    matrix: found.shown.then_some(found.matrix),
                    mask: found.mask,
                }
            })
            .collect();
        pages.push((page, placed));
    }
    Ok(Placements {
        images: order,
        pages,
    })
}

/// The asset name of the image of `index`, without its extension.
pub fn stem(index: usize) -> String {
    format!("image-{:03}", index + 1)
}

/// The images the pages of `doc` draw as files, named by their index, so
/// that a name says which image it is whether or not the images before it
/// could be made files.
pub(super) fn read(doc: &Pdf, max_page: usize) -> Result<ImageAssets> {
    let Placements {
        images: order,
        pages: placed,
    } = placements(doc, max_page)?;
    let mut drawn_on: Vec<Vec<u32>> = vec![vec![]; order.len()];
    for (page, placements) in &placed {
        for placement in placements {
            let pages = &mut drawn_on[placement.index];
            if pages.last() != Some(page) {
                pages.push(*page);
            }
        }
    }
    let mut images = ImageAssets::default();
    let mut left_out: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for (index, (image, pages)) in order.into_iter().zip(drawn_on).enumerate() {
        let file = match &image {
            Drawn::Composite(pieces) => compose(doc, pieces).map(|pixels| (pixels.png(), "png")),
            image => source(doc, image).and_then(|(stream, dict)| image_file(doc, stream, &dict)),
        };
        match file {
            Ok((bytes, extension)) => {
                let name = format!("{}.{extension}", stem(index));
                images.files.insert(name.clone(), bytes);
                images.pages.insert(name, pages);
            }
            Err(error) => {
                for page in pages {
                    left_out
                        .entry(page)
                        .or_default()
                        .insert(format!("{error:#}"));
                }
            }
        }
    }
    if !left_out.is_empty() {
        let pages = left_out
            .iter()
            .map(|(page, reasons)| {
                format!(
                    "{page}ページ（{}）",
                    reasons.iter().cloned().collect::<Vec<_>>().join("、")
                )
            })
            .collect::<Vec<_>>()
            .join("、");
        images.note = format!(
            "次のページの画像は画像ファイルにできないため、assetsに保存せずOCRもしていません: {pages}。この画像の文字を読むには、画像をPNGかJPEGにしたPDFか、OCRで文字を付けたPDFを取り込んでください。"
        );
    }
    Ok(images)
}

/// `object` as a dictionary, following a reference.
fn dictionary<'a>(doc: &'a Pdf, object: &'a Object) -> Option<&'a Dictionary> {
    match object {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        object => object.as_dict().ok(),
    }
}

/// `object`, following references.
fn resolved<'a>(doc: &'a Pdf, object: &'a Object) -> &'a Object {
    doc.dereference(object).map_or(object, |(_, object)| object)
}

const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// The six numbers of a matrix written as operands or an array.
fn matrix(numbers: &[Object]) -> Option<[f64; 6]> {
    let numbers: Vec<f64> = numbers
        .iter()
        .map(|n| n.as_float().map(f64::from))
        .collect::<Result<_, _>>()
        .ok()?;
    numbers.try_into().ok()
}

/// The text state that places the glyphs a string shows: its font (by its
/// resource name) and size, character and word spacing, horizontal scaling
/// as a fraction, leading and rise.
#[derive(Clone)]
struct TextState {
    font: Option<Vec<u8>>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            font: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
        }
    }
}

/// A glyph procedure of a Type3 font that draws an image, read: its content
/// and the operations of it.
type Procedure = std::rc::Rc<(Vec<u8>, Content)>;

/// Walks what a page draws, finding the images it draws.
struct Walker<'a> {
    doc: &'a Pdf,
    /// The forms, patterns and glyph procedures being walked, which may not
    /// draw themselves.
    open: BTreeSet<ObjectId>,
    /// The patterns and soft mask groups walked: the images of each are
    /// found once on a page however often it is used.
    walked: BTreeSet<ObjectId>,
    /// The glyph procedures read, none for one that draws no image.
    procedures: BTreeMap<ObjectId, Option<Procedure>>,
    /// The widths of the fonts strings are shown in, by font.
    widths: BTreeMap<usize, super::PdfWidths>,
    /// Whether the images found now show where they are drawn, and whether
    /// they are drawn by glyphs.
    shown: bool,
    glyph: bool,
    found: Vec<Found>,
}

/// The entry `name` of the resources of `kind` (XObject, Pattern, ...),
/// from the first of `resources` that has it.
fn resource<'a>(
    doc: &'a Pdf,
    resources: &[&'a Dictionary],
    kind: &[u8],
    name: &[u8],
) -> Option<&'a Object> {
    resources
        .iter()
        .find_map(|r| dictionary(doc, r.get(kind).ok()?)?.get(name).ok())
}

/// Whether a color space name is that of a device or CIE family, which an
/// image may name without resources.
fn family(name: &[u8]) -> bool {
    matches!(
        name,
        b"DeviceGray"
            | b"DeviceRGB"
            | b"DeviceCMYK"
            | b"CalGray"
            | b"CalRGB"
            | b"G"
            | b"RGB"
            | b"CMYK"
    )
}

impl<'a> Walker<'a> {
    fn new(doc: &'a Pdf) -> Self {
        Self {
            doc,
            open: BTreeSet::new(),
            walked: BTreeSet::new(),
            procedures: BTreeMap::new(),
            widths: BTreeMap::new(),
            shown: true,
            glyph: false,
            found: vec![],
        }
    }

    fn push(&mut self, image: Drawn, matrix: [f64; 6], mask: bool) {
        self.found.push(Found {
            image,
            matrix,
            mask,
            shown: self.shown,
            glyph: self.glyph,
        });
    }

    /// The images `content` (decoded from `raw`) draws when `base` maps its
    /// space onto the page: with `Do` and inline, and in the Form XObjects,
    /// tiling patterns and Type3 glyphs it draws and the soft masks it sets.
    fn content(
        &mut self,
        content: &Content,
        raw: &[u8],
        resources: &[&'a Dictionary],
        base: [f64; 6],
        depth: usize,
    ) {
        let doc = self.doc;
        let mut ctm = base;
        let mut text = TextState::default();
        // The text matrix and the start of its line, which Td, TD and T*
        // move from.
        let (mut tm, mut tlm) = (IDENTITY, IDENTITY);
        let mut saved = vec![];
        // The byte ranges of the inline images, found when the first is drawn.
        let mut inline_ranges = None;
        let mut inline_count = 0;
        for operation in &content.operations {
            let operands = &operation.operands;
            let number = |i: usize| {
                operands
                    .get(i)
                    .and_then(|n| n.as_float().ok())
                    .map(f64::from)
            };
            let next_line = |text: &TextState, tlm: &mut [f64; 6]| {
                *tlm = super::pdf_multiply([1.0, 0.0, 0.0, 1.0, 0.0, -text.leading], *tlm);
                *tlm
            };
            match operation.operator.as_str() {
                "q" => saved.push((ctm, text.clone())),
                "Q" => {
                    if let Some(state) = saved.pop() {
                        (ctm, text) = state;
                    }
                }
                "cm" => {
                    if let Some(m) = matrix(operands) {
                        ctm = super::pdf_multiply(m, ctm);
                    }
                }
                "BT" => (tm, tlm) = (IDENTITY, IDENTITY),
                "Tm" => {
                    if let Some(m) = matrix(operands) {
                        (tm, tlm) = (m, m);
                    }
                }
                "Td" | "TD" => {
                    if let (Some(x), Some(y)) = (number(0), number(1)) {
                        if operation.operator == "TD" {
                            text.leading = -y;
                        }
                        tlm = super::pdf_multiply([1.0, 0.0, 0.0, 1.0, x, y], tlm);
                        tm = tlm;
                    }
                }
                "T*" => tm = next_line(&text, &mut tlm),
                "TL" => text.leading = number(0).unwrap_or(text.leading),
                "Tc" => text.char_spacing = number(0).unwrap_or(text.char_spacing),
                "Tw" => text.word_spacing = number(0).unwrap_or(text.word_spacing),
                "Tz" => text.scale = number(0).map_or(text.scale, |s| s / 100.0),
                "Ts" => text.rise = number(0).unwrap_or(text.rise),
                "Tf" => {
                    text.font = operands
                        .first()
                        .and_then(|n| n.as_name().ok())
                        .map(<[u8]>::to_vec);
                    text.size = number(1).unwrap_or(text.size);
                }
                "Tj" | "TJ" | "'" | "\"" => {
                    if operation.operator == "\"" {
                        text.word_spacing = number(0).unwrap_or(text.word_spacing);
                        text.char_spacing = number(1).unwrap_or(text.char_spacing);
                    }
                    if matches!(operation.operator.as_str(), "'" | "\"") {
                        tm = next_line(&text, &mut tlm);
                    }
                    self.show(operands.last(), &text, &mut tm, ctm, resources, depth);
                }
                "BI" => {
                    let ranges = inline_ranges.get_or_insert_with(|| {
                        let lengths: Vec<_> = content
                            .operations
                            .iter()
                            .filter(|o| o.operator == "BI")
                            .map(|o| match o.operands.first() {
                                Some(Object::Stream(stream)) => Some(stream.content.len()),
                                _ => None,
                            })
                            .collect();
                        super::inline_images(raw, &lengths).unwrap_or_default()
                    });
                    let range = ranges.get(inline_count).cloned();
                    inline_count += 1;
                    if let Some(stream) = range.and_then(|range| {
                        inline_image(doc, &raw[range], operands.first(), resources)
                    }) {
                        let mask = boolean(&stream.dict, b"ImageMask").unwrap_or(false);
                        self.push(Drawn::Inline(Box::new(stream)), ctm, mask);
                    }
                }
                "Do" => {
                    let Some(Ok(name)) = operands.first().map(Object::as_name) else {
                        continue;
                    };
                    let Some(Ok(id)) =
                        resource(doc, resources, b"XObject", name).map(Object::as_reference)
                    else {
                        continue;
                    };
                    let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
                        continue;
                    };
                    match stream.dict.get(b"Subtype").and_then(Object::as_name) {
                        Ok(b"Image") => {
                            let mask = boolean(&stream.dict, b"ImageMask").unwrap_or(false);
                            // A color space named in the resources, as some
                            // writers name one an image uses.
                            let space = match stream.dict.get(b"ColorSpace") {
                                Ok(Object::Name(name)) if !family(name) => {
                                    resource(doc, resources, b"ColorSpace", name)
                                        .map(|space| Box::new(space.clone()))
                                }
                                _ => None,
                            };
                            self.push(Drawn::Object(id, space), ctm, mask);
                        }
                        Ok(b"Form") => self.form(id, resources, ctm, depth),
                        _ => {}
                    }
                }
                // A tiling pattern paints its cell over and over: its images
                // are found once, not shown where they are drawn. Its matrix
                // maps its space onto that of the content it is used in.
                "scn" | "SCN" => {
                    let Some(Ok(name)) = operands.last().map(Object::as_name) else {
                        continue;
                    };
                    let Some(Ok(id)) =
                        resource(doc, resources, b"Pattern", name).map(Object::as_reference)
                    else {
                        continue;
                    };
                    let tiling = doc
                        .get_object(id)
                        .and_then(Object::as_stream)
                        .is_ok_and(|p| integer(&p.dict, b"PatternType") == Some(1));
                    if tiling && self.walked.insert(id) {
                        let shown = std::mem::replace(&mut self.shown, false);
                        self.form(id, resources, base, depth);
                        self.shown = shown;
                    }
                }
                // The group of a soft mask is drawn only for its shape.
                "gs" => {
                    let Some(Ok(name)) = operands.first().map(Object::as_name) else {
                        continue;
                    };
                    let Some(group) = resource(doc, resources, b"ExtGState", name)
                        .and_then(|state| dictionary(doc, state))
                        .and_then(|state| dictionary(doc, state.get(b"SMask").ok()?))
                        .and_then(|mask| mask.get(b"G").ok()?.as_reference().ok())
                    else {
                        continue;
                    };
                    if self.walked.insert(group) {
                        let shown = std::mem::replace(&mut self.shown, false);
                        self.form(group, resources, ctm, depth);
                        self.shown = shown;
                    }
                }
                _ => {}
            }
        }
    }

    /// The images the Form XObject (or tiling pattern) `id` draws when `ctm`
    /// maps the space it is drawn in onto the page, after its own matrix;
    /// `resources` are those of what draws it.
    fn form(&mut self, id: ObjectId, resources: &[&'a Dictionary], ctm: [f64; 6], depth: usize) {
        let doc = self.doc;
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
            return;
        };
        if depth >= MAX_FORM_DEPTH || !self.open.insert(id) {
            return;
        }
        if let Ok(raw) = stream.decompressed_content_with_limit(MAX_SAMPLES)
            && let Ok(inner) = operations(&raw)
        {
            // A form without resources of its own uses those of the page
            // drawing it, as older PDFs write.
            let own = stream
                .dict
                .get(b"Resources")
                .ok()
                .and_then(|r| dictionary(doc, r));
            let resources = match own {
                Some(own) => vec![own],
                None => resources.to_vec(),
            };
            self.content(
                &inner,
                &raw,
                &resources,
                super::pdf_multiply(form_matrix(doc, &stream.dict), ctm),
                depth + 1,
            );
        }
        self.open.remove(&id);
    }

    /// Moves the text matrix `tm` along the string (or TJ array) `operand`
    /// shows, finding the images its glyphs draw when its font is a Type3
    /// font.
    fn show(
        &mut self,
        operand: Option<&Object>,
        text: &TextState,
        tm: &mut [f64; 6],
        ctm: [f64; 6],
        resources: &[&'a Dictionary],
        depth: usize,
    ) {
        let doc = self.doc;
        let Some(font) = text
            .font
            .as_ref()
            .and_then(|name| resource(doc, resources, b"Font", name))
            .and_then(|font| dictionary(doc, font))
        else {
            return;
        };
        let items: Vec<&Object> = match operand {
            Some(Object::Array(items)) => items.iter().collect(),
            Some(string @ Object::String(..)) => vec![string],
            _ => return,
        };
        let key = super::font_key(font);
        let code = self
            .widths
            .entry(key)
            .or_insert_with(|| super::PdfWidths::for_font(font, doc))
            .code;
        let type3 = font
            .get(b"Subtype")
            .and_then(Object::as_name)
            .is_ok_and(|s| s == b"Type3");
        for item in items {
            let Object::String(bytes, _) = item else {
                // A number of a TJ array moves the next string back by
                // thousandths of the text size.
                if let Ok(adjustment) = item.as_float() {
                    let tx = -f64::from(adjustment) / 1000.0 * text.size * text.scale;
                    *tm = super::pdf_multiply([1.0, 0.0, 0.0, 1.0, tx, 0.0], *tm);
                }
                continue;
            };
            for glyph in bytes.chunks(code) {
                if type3 {
                    self.glyph(font, glyph[0], text, *tm, ctm, resources, depth);
                }
                let width = self.widths.get(&key).map_or(0.0, |w| w.advance(glyph));
                let space = if glyph == b" " {
                    text.word_spacing
                } else {
                    0.0
                };
                let tx = (width / 1000.0 * text.size + text.char_spacing + space) * text.scale;
                *tm = super::pdf_multiply([1.0, 0.0, 0.0, 1.0, tx, 0.0], *tm);
            }
        }
    }

    /// The images the glyph of `code` of the Type3 font `font` draws when
    /// shown at `tm` in the space `ctm` maps onto the page.
    #[allow(clippy::too_many_arguments)]
    fn glyph(
        &mut self,
        font: &'a Dictionary,
        code: u8,
        text: &TextState,
        tm: [f64; 6],
        ctm: [f64; 6],
        resources: &[&'a Dictionary],
        depth: usize,
    ) {
        let doc = self.doc;
        let Some(id) = glyph_name(doc, font, code).and_then(|name| {
            dictionary(doc, font.get(b"CharProcs").ok()?)?
                .get(&name)
                .ok()?
                .as_reference()
                .ok()
        }) else {
            return;
        };
        let procedure = match self.procedures.get(&id) {
            Some(procedure) => procedure.clone(),
            None => {
                let procedure = procedure(doc, id);
                self.procedures.insert(id, procedure.clone());
                procedure
            }
        };
        let Some(procedure) = procedure else {
            return;
        };
        if depth >= MAX_FORM_DEPTH || !self.open.insert(id) {
            return;
        }
        let font_matrix = font
            .get(b"FontMatrix")
            .ok()
            .and_then(|m| numbers(doc, m))
            .and_then(|m| <[f64; 6]>::try_from(m).ok())
            .unwrap_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
        let rendering = [text.size * text.scale, 0.0, 0.0, text.size, 0.0, text.rise];
        let onto = super::pdf_multiply(
            font_matrix,
            super::pdf_multiply(rendering, super::pdf_multiply(tm, ctm)),
        );
        // A Type3 font without resources of its own uses those of the page.
        let own = font.get(b"Resources").ok().and_then(|r| dictionary(doc, r));
        let resources = match own {
            Some(own) => vec![own],
            None => resources.to_vec(),
        };
        let glyph = std::mem::replace(&mut self.glyph, true);
        self.content(&procedure.1, &procedure.0, &resources, onto, depth + 1);
        self.glyph = glyph;
        self.open.remove(&id);
    }
}

/// The name the encoding of the Type3 font `font` gives `code`.
fn glyph_name(doc: &Pdf, font: &Dictionary, code: u8) -> Option<Vec<u8>> {
    let differences = dictionary(doc, font.get(b"Encoding").ok()?)?
        .get(b"Differences")
        .ok()
        .map(|d| resolved(doc, d))?
        .as_array()
        .ok()?;
    let mut next = None;
    let mut name = None;
    for entry in differences {
        match resolved(doc, entry) {
            Object::Integer(n) => next = Some(*n),
            Object::Name(glyph) => {
                if next == Some(i64::from(code)) {
                    name = Some(glyph.clone());
                }
                next = next.map(|n| n + 1);
            }
            _ => {}
        }
    }
    name
}

/// The glyph procedure `id` read, when it draws an image.
fn procedure(doc: &Pdf, id: ObjectId) -> Option<Procedure> {
    let stream = doc.get_object(id).and_then(Object::as_stream).ok()?;
    let raw = stream.decompressed_content_with_limit(MAX_SAMPLES).ok()?;
    let draws = |word: &[u8]| raw.windows(2).any(|w| w == word);
    if !draws(b"Do") && !draws(b"BI") {
        return None;
    }
    let content = operations(&raw).ok()?;
    Some(std::rc::Rc::new((raw, content)))
}

/// The operations of content `raw` that place images. A stream lopdf cannot
/// read to its end (a stray delimiter, a broken string) is read by
/// [`tolerant`], which steps over what it cannot read, as PDF readers do.
fn operations(raw: &[u8]) -> Result<Content> {
    let (content, whole) = super::pdf_content(raw)?;
    Ok(if whole { content } else { tolerant(raw) })
}

/// The operations of content `raw`, read token by token: a token that
/// cannot be read is stepped over with the operands before it. Strings,
/// arrays and dictionaries are kept as nulls, since placing an image needs
/// only numbers and names; an inline image is a BI without operands, whose
/// data is stepped over.
fn tolerant(raw: &[u8]) -> Content {
    let space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0');
    let delimiter = |b: u8| space(b) || b"()<>[]{}/%".contains(&b);
    let mut operations = vec![];
    let mut operands = vec![];
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        if space(b) {
            i += 1;
            continue;
        }
        let end = |from: usize| {
            raw[from..]
                .iter()
                .position(|b| delimiter(*b))
                .map_or(raw.len(), |p| from + p)
        };
        match b {
            b'%' => {
                i = raw[i..]
                    .iter()
                    .position(|b| matches!(b, b'\r' | b'\n'))
                    .map_or(raw.len(), |p| i + p);
            }
            b'/' => {
                let to = end(i + 1);
                operands.push(Object::Name(raw[i + 1..to].to_vec()));
                i = to;
            }
            b'(' | b'[' | b'<' => {
                let to = if b == b'(' {
                    let (mut depth, mut j) = (0usize, i);
                    while j < raw.len() {
                        match raw[j] {
                            b'\\' => j += 1,
                            b'(' => depth += 1,
                            b')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    j + 1
                } else {
                    super::pdf_value_end(&raw[i..]).map_or(raw.len(), |n| i + n)
                };
                operands.push(Object::Null);
                i = to.min(raw.len());
            }
            b')' | b']' | b'>' | b'{' | b'}' => {
                operands.clear();
                i += 1;
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                let to = end(i).max(i + 1);
                match std::str::from_utf8(&raw[i..to])
                    .ok()
                    .and_then(|n| n.parse::<f32>().ok())
                {
                    Some(n) => operands.push(Object::Real(n)),
                    None => operands.clear(),
                }
                i = to;
            }
            _ => {
                let to = end(i).max(i + 1);
                let word = &raw[i..to];
                if word == b"BI" {
                    // Past the image: its data, then EI.
                    let id = (to..raw.len().saturating_sub(1))
                        .find(|&at| raw[at..].starts_with(b"ID") && space(raw[at - 1]));
                    let data = id.map(|id| id + 3);
                    let length = id.and_then(|id| super::inline_length(&raw[to..id]));
                    i = match (data, length) {
                        (Some(data), Some(length)) => data + length,
                        (Some(data), None) => (data..raw.len().saturating_sub(1))
                            .find(|&at| space(raw[at - 1]) && raw[at..].starts_with(b"EI"))
                            .unwrap_or(raw.len()),
                        _ => raw.len(),
                    };
                    operands.clear();
                    operations.push(lopdf::content::Operation::new("BI", vec![]));
                    continue;
                }
                operations.push(lopdf::content::Operation::new(
                    &String::from_utf8_lossy(word),
                    std::mem::take(&mut operands),
                ));
                i = to;
            }
        }
    }
    Content { operations }
}

/// A form's Matrix, the identity when it has none.
fn form_matrix(doc: &Pdf, dict: &Dictionary) -> [f64; 6] {
    dict.get(b"Matrix")
        .ok()
        .and_then(|m| resolved(doc, m).as_array().ok())
        .and_then(|m| matrix(m))
        .unwrap_or(IDENTITY)
}

/// The numbers of an array, following references.
fn numbers(doc: &Pdf, object: &Object) -> Option<Vec<f64>> {
    resolved(doc, object)
        .as_array()
        .ok()?
        .iter()
        .map(|n| resolved(doc, n).as_float().ok().map(f64::from))
        .collect()
}

/// The appearances the annotations of page `id` show, as a viewer shows
/// them: the normal appearance (the one its state names), with the matrix
/// that maps the space it is drawn in onto the page, its box onto the
/// annotation's rectangle, and whether it is a button's (a check box, a
/// radio button, a push button), which shows a mark rather than text.
/// Hidden annotations are left out, as are those not shown on screen.
pub(super) fn appearances(doc: &Pdf, id: ObjectId) -> Vec<(ObjectId, [f64; 6], bool)> {
    let mut shown = vec![];
    let Some(annots) = doc
        .get_dictionary(id)
        .ok()
        .and_then(|page| page.get(b"Annots").ok())
        .and_then(|a| resolved(doc, a).as_array().ok())
    else {
        return shown;
    };
    for annot in annots {
        let Some(annot) = dictionary(doc, annot) else {
            continue;
        };
        // Hidden (2) and NoView (32).
        let flags = annot
            .get(b"F")
            .ok()
            .and_then(|f| resolved(doc, f).as_i64().ok())
            .unwrap_or(0);
        if flags & (2 | 32) != 0 {
            continue;
        }
        let Some(normal) = annot
            .get(b"AP")
            .ok()
            .and_then(|ap| dictionary(doc, ap))
            .and_then(|ap| ap.get(b"N").ok())
        else {
            continue;
        };
        let appearance = match normal {
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(Object::Stream(_)) => Some(*id),
                // A dictionary of appearances by state: the one AS names.
                Ok(Object::Dictionary(states)) => annot
                    .get(b"AS")
                    .ok()
                    .and_then(|state| resolved(doc, state).as_name().ok())
                    .and_then(|state| states.get(state).ok())
                    .and_then(|a| a.as_reference().ok()),
                _ => None,
            },
            Object::Dictionary(states) => annot
                .get(b"AS")
                .ok()
                .and_then(|state| resolved(doc, state).as_name().ok())
                .and_then(|state| states.get(state).ok())
                .and_then(|a| a.as_reference().ok()),
            _ => None,
        };
        let Some(appearance) = appearance else {
            continue;
        };
        let Ok(stream) = doc.get_object(appearance).and_then(Object::as_stream) else {
            continue;
        };
        let (Some(rect), Some(bbox)) = (
            annot.get(b"Rect").ok().and_then(|r| numbers(doc, r)),
            stream.dict.get(b"BBox").ok().and_then(|b| numbers(doc, b)),
        ) else {
            continue;
        };
        let (Ok(rect), Ok(bbox)) = (<[f64; 4]>::try_from(rect), <[f64; 4]>::try_from(bbox)) else {
            continue;
        };
        // The box as the form's matrix draws it, then that mapped onto the
        // rectangle.
        let m = form_matrix(doc, &stream.dict);
        let corners = [
            [bbox[0], bbox[1]],
            [bbox[2], bbox[1]],
            [bbox[0], bbox[3]],
            [bbox[2], bbox[3]],
        ]
        .map(|[x, y]| [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]);
        let fold = |axis: usize, pick: fn(f64, f64) -> f64| {
            corners.iter().map(|c| c[axis]).fold(corners[0][axis], pick)
        };
        let (bx0, by0, bx1, by1) = (
            fold(0, f64::min),
            fold(1, f64::min),
            fold(0, f64::max),
            fold(1, f64::max),
        );
        let (rx0, ry0) = (rect[0].min(rect[2]), rect[1].min(rect[3]));
        let (rx1, ry1) = (rect[0].max(rect[2]), rect[1].max(rect[3]));
        if bx1 - bx0 <= 0.0 || by1 - by0 <= 0.0 {
            continue;
        }
        let (sx, sy) = ((rx1 - rx0) / (bx1 - bx0), (ry1 - ry0) / (by1 - by0));
        let onto = [sx, 0.0, 0.0, sy, rx0 - bx0 * sx, ry0 - by0 * sy];
        // The field type, which a widget may take from its field.
        let mut field = Some(annot);
        let mut button = false;
        for _ in 0..8 {
            let Some(node) = field else { break };
            if let Ok(kind) = node.get(b"FT").map(|t| resolved(doc, t)) {
                button = kind.as_name().is_ok_and(|k| k == b"Btn");
                break;
            }
            field = node.get(b"Parent").ok().and_then(|p| dictionary(doc, p));
        }
        shown.push((appearance, onto, button));
    }
    shown
}

/// The inline image `bytes` (`BI` to `EI`) holds, with its dictionary written
/// out as an image XObject's (full keys and filter and color space names, a
/// named color space taken from `resources`); none when it is too small to
/// hold text or cannot be read. `parsed` is what lopdf read of it: its data
/// when it could tell where it ends.
fn inline_image(
    doc: &Pdf,
    bytes: &[u8],
    parsed: Option<&Object>,
    resources: &[&Dictionary],
) -> Option<Stream> {
    let space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0');
    // The ID that ends the dictionary: the first set off by whitespace.
    let id = (2..bytes.len().saturating_sub(2))
        .find(|&i| space(bytes[i - 1]) && bytes[i..].starts_with(b"ID") && space(bytes[i + 2]))?;
    let mut written = b"<<".to_vec();
    written.extend_from_slice(&bytes[2..id]);
    written.extend_from_slice(b">> BI");
    let abbreviated = Content::decode(&written)
        .ok()?
        .operations
        .into_iter()
        .next()?
        .operands
        .into_iter()
        .next()?
        .as_dict()
        .ok()?
        .clone();
    let data = match parsed {
        Some(Object::Stream(stream)) => stream.content.clone(),
        // The data runs from after the whitespace that ends ID to the
        // whitespace before EI.
        _ => {
            let end = bytes.len().checked_sub(2)?;
            let end = if end > id + 3 && space(bytes[end - 1]) {
                end - 1
            } else {
                end
            };
            bytes.get(id + 3..end)?.to_vec()
        }
    };
    let full_name = |name: &[u8]| -> Vec<u8> {
        match name {
            b"AHx" => b"ASCIIHexDecode".to_vec(),
            b"A85" => b"ASCII85Decode".to_vec(),
            b"LZW" => b"LZWDecode".to_vec(),
            b"Fl" => b"FlateDecode".to_vec(),
            b"RL" => b"RunLengthDecode".to_vec(),
            b"CCF" => b"CCITTFaxDecode".to_vec(),
            b"DCT" => b"DCTDecode".to_vec(),
            other => other.to_vec(),
        }
    };
    let mut dict = Dictionary::new();
    dict.set("Subtype", Object::Name(b"Image".to_vec()));
    for (key, value) in abbreviated.iter() {
        let key: &[u8] = match key.as_slice() {
            b"BPC" => b"BitsPerComponent",
            b"CS" => b"ColorSpace",
            b"D" => b"Decode",
            b"DP" => b"DecodeParms",
            b"F" => b"Filter",
            b"H" => b"Height",
            b"IM" => b"ImageMask",
            b"I" => b"Interpolate",
            b"W" => b"Width",
            other => other,
        };
        let value = match (key, value) {
            (b"Filter", Object::Name(name)) => Object::Name(full_name(name)),
            (b"Filter", Object::Array(names)) => Object::Array(
                names
                    .iter()
                    .map(|n| match n {
                        Object::Name(name) => Object::Name(full_name(name)),
                        other => other.clone(),
                    })
                    .collect(),
            ),
            // A color space other than a device space is named in the
            // page's resources.
            (b"ColorSpace", Object::Name(name))
                if !matches!(
                    name.as_slice(),
                    b"G" | b"RGB" | b"CMYK" | b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"
                ) =>
            {
                resources
                    .iter()
                    .find_map(|r| dictionary(doc, r.get(b"ColorSpace").ok()?)?.get(name).ok())
                    .cloned()
                    .unwrap_or_else(|| value.clone())
            }
            _ => value.clone(),
        };
        dict.set(key.to_vec(), value);
    }
    integer(&dict, b"Width")?;
    integer(&dict, b"Height")?;
    Some(Stream::new(dict, data))
}

/// Whether an inline image is too small to hold text.
fn small(dict: &Dictionary) -> bool {
    let (width, height) = (
        integer(dict, b"Width").unwrap_or(0),
        integer(dict, b"Height").unwrap_or(0),
    );
    width.min(height) < MIN_INLINE_SIDE || width * height < MIN_INLINE_PIXELS
}

/// The images of a page, found in the order it draws them, as files are
/// made of them: the images the glyphs of its Type3 fonts draw made one
/// picture, as are images drawn edge to edge as the strips or tiles of a
/// picture; inline images too small to hold text left out.
fn gathered(doc: &Pdf, found: Vec<Found>) -> Vec<Found> {
    let glyphs = found.iter().filter(|f| f.glyph && f.shown).count();
    let found = if glyphs > 1 {
        let mut kept = vec![];
        let mut pieces = vec![];
        let mut at = 0;
        for f in found {
            if f.glyph && f.shown {
                if pieces.is_empty() {
                    at = kept.len();
                }
                pieces.push(Piece {
                    image: f.image,
                    matrix: f.matrix,
                    mask: f.mask,
                });
            } else {
                kept.push(f);
            }
        }
        kept.insert(at, composite(pieces));
        kept
    } else {
        found
    };
    let mut found = joined(doc, found);
    found.retain(|f| !matches!(&f.image, Drawn::Inline(stream) if small(&stream.dict)));
    found
}

/// The box `matrix` maps the unit square onto: `[x0, y0, x1, y1]`.
fn bounds(matrix: [f64; 6]) -> [f64; 4] {
    let [a, b, c, d, e, f] = matrix;
    let xs = [e, a + e, c + e, a + c + e];
    let ys = [f, b + f, d + f, b + d + f];
    [
        xs.into_iter().fold(f64::INFINITY, f64::min),
        ys.into_iter().fold(f64::INFINITY, f64::min),
        xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
        ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
    ]
}

/// The pieces as one image, drawn over the box they cover.
fn composite(pieces: Vec<Piece>) -> Found {
    let [x0, y0, x1, y1] = pieces
        .iter()
        .map(|p| bounds(p.matrix))
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
        .unwrap_or([0.0; 4]);
    Found {
        image: Drawn::Composite(pieces),
        matrix: [x1 - x0, 0.0, 0.0, y1 - y0, x0, y0],
        mask: false,
        shown: true,
        glyph: false,
    }
}

/// The most images of a page tried for strips: pages drawing more are left
/// as they are.
const MAX_STRIP_CANDIDATES: usize = 4000;

/// `found` with the images drawn edge to edge as the strips or tiles of one
/// picture made one: images upright on the page, each sharing a whole edge
/// with another, with as many pixels along it, together covering a box
/// without gaps or overlaps. The picture is where its first part is drawn.
fn joined(doc: &Pdf, found: Vec<Found>) -> Vec<Found> {
    // The box and the pixel width and height of each image that may be a part.
    let parts: Vec<Option<([f64; 4], i64, i64)>> = found
        .iter()
        .map(|f| {
            let [a, b, c, d, ..] = f.matrix;
            let upright = b.abs() <= 1e-6 * a.abs() && c.abs() <= 1e-6 * d.abs();
            if !f.shown || f.glyph || !upright || a == 0.0 || d == 0.0 {
                return None;
            }
            let dict = match &f.image {
                Drawn::Object(id, _) => &doc.get_object(*id).and_then(Object::as_stream).ok()?.dict,
                Drawn::Inline(stream) => &stream.dict,
                Drawn::Composite(_) => return None,
            };
            let size = |key: &[u8]| resolved(doc, dict.get(key).ok()?).as_i64().ok();
            Some((bounds(f.matrix), size(b"Width")?, size(b"Height")?))
        })
        .collect();
    let candidates: Vec<usize> = (0..found.len()).filter(|i| parts[*i].is_some()).collect();
    if candidates.len() < 2 || candidates.len() > MAX_STRIP_CANDIDATES {
        return found;
    }
    // Images sharing an edge are joined, as sets by their first image.
    let mut parent: Vec<usize> = (0..found.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let near = |p: f64, q: f64, extent: f64| (p - q).abs() <= (0.01 * extent).max(0.05);
    for (n, &i) in candidates.iter().enumerate() {
        for &j in &candidates[n + 1..] {
            let (Some((r, w, h)), Some((s, v, g))) = (parts[i], parts[j]) else {
                continue;
            };
            if found[i].mask != found[j].mask {
                continue;
            }
            let width = (r[2] - r[0]).min(s[2] - s[0]);
            let height = (r[3] - r[1]).min(s[3] - s[1]);
            let column = w == v
                && near(r[0], s[0], width)
                && near(r[2], s[2], width)
                && (near(r[3], s[1], height) || near(s[3], r[1], height));
            let row = h == g
                && near(r[1], s[1], height)
                && near(r[3], s[3], height)
                && (near(r[2], s[0], width) || near(s[2], r[0], width));
            if column || row {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut sets: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &i in &candidates {
        let first = root(&mut parent, i);
        sets.entry(first).or_default().push(i);
    }
    // A set covering its box once, as the parts of a picture do.
    let mut joined: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (first, members) in sets {
        if members.len() < 2 {
            continue;
        }
        let boxes: Vec<[f64; 4]> = members
            .iter()
            .filter_map(|i| parts[*i])
            .map(|p| p.0)
            .collect();
        let area = |r: &[f64; 4]| (r[2] - r[0]) * (r[3] - r[1]);
        let whole = boxes
            .iter()
            .copied()
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            })
            .unwrap_or([0.0; 4]);
        let covered: f64 = boxes.iter().map(area).sum();
        if (covered - area(&whole)).abs() <= 0.02 * area(&whole) {
            joined.insert(first, members);
        }
    }
    if joined.is_empty() {
        return found;
    }
    let part_of: BTreeMap<usize, usize> = joined
        .iter()
        .flat_map(|(first, members)| members.iter().map(move |m| (*m, *first)))
        .collect();
    let mut pieces: BTreeMap<usize, Vec<Piece>> = BTreeMap::new();
    let mut kept = vec![];
    let mut order = vec![];
    for (i, f) in found.into_iter().enumerate() {
        match part_of.get(&i) {
            Some(first) => {
                if *first == i {
                    order.push((kept.len(), i));
                }
                pieces.entry(*first).or_default().push(Piece {
                    image: f.image,
                    matrix: f.matrix,
                    mask: f.mask,
                });
            }
            None => kept.push(f),
        }
    }
    // Each picture where its first part was, the later first.
    for (at, first) in order.into_iter().rev() {
        let parts = pieces.remove(&first).unwrap_or_default();
        kept.insert(at, composite(parts));
    }
    kept
}

fn integer(dict: &Dictionary, key: &[u8]) -> Option<i64> {
    dict.get(key).and_then(Object::as_i64).ok()
}

fn boolean(dict: &Dictionary, key: &[u8]) -> Option<bool> {
    dict.get(key).and_then(Object::as_bool).ok()
}

/// `dict` with the references of its entries followed, except to streams
/// (a soft mask, a mask, an ICC profile), which stay references: a writer
/// may put an image's width or filter in an object of its own.
fn flat(doc: &Pdf, dict: &Dictionary) -> Dictionary {
    let mut flat = dict.clone();
    for (_, value) in flat.iter_mut() {
        if let Object::Reference(id) = value
            && let Ok(target) = doc.get_object(*id)
            && !matches!(target, Object::Stream(_))
        {
            *value = target.clone();
        }
    }
    flat
}

/// The stream of an image XObject or inline image, and its dictionary with
/// its references followed and the color space named in resources put in.
fn source<'a>(doc: &'a Pdf, image: &'a Drawn) -> Result<(&'a Stream, Dictionary)> {
    match image {
        Drawn::Object(id, space) => {
            let stream = doc.get_object(*id).and_then(Object::as_stream)?;
            let mut dict = flat(doc, &stream.dict);
            if let Some(space) = space {
                dict.set("ColorSpace", (**space).clone());
            }
            Ok((stream, dict))
        }
        Drawn::Inline(stream) => Ok((stream, flat(doc, &stream.dict))),
        Drawn::Composite(_) => bail!("画像を読めない"),
    }
}

/// The file of an image, of `stream` and `dict`, and its extension.
fn image_file(doc: &Pdf, stream: &Stream, dict: &Dictionary) -> Result<(Vec<u8>, &'static str)> {
    match decoded(doc, dict, &stream.content)? {
        Decoded::Jpeg(bytes) => Ok((bytes, "jpg")),
        Decoded::Pixels(mut pixels) => {
            if pixels.alpha.is_none() {
                masked(doc, dict, &mut pixels);
            }
            Ok((pixels.png(), "png"))
        }
    }
}

/// The pixels of an image, with the transparency its masks give it.
fn image_pixels(doc: &Pdf, image: &Drawn) -> Result<Pixels> {
    let (stream, dict) = source(doc, image)?;
    let mut pixels = match decoded(doc, &dict, &stream.content)? {
        Decoded::Jpeg(bytes) => jpeg_pixels(doc, &dict, &bytes)?,
        Decoded::Pixels(pixels) => pixels,
    };
    if pixels.alpha.is_none() {
        masked(doc, &dict, &mut pixels);
    }
    Ok(pixels)
}

/// The longest side, and the most pixels, of an image made of pieces.
const MAX_COMPOSITE_SIDE: f64 = 8000.0;
const MAX_COMPOSITE_PIXELS: f64 = 32_000_000.0;

/// The pieces drawn on one image as the page draws them, over the box they
/// cover, as finely as the finest of them; transparent where none draws. A
/// stencil mask paints black. Pieces that cannot be read are left out.
fn compose(doc: &Pdf, pieces: &[Piece]) -> Result<Pixels> {
    let mut read = vec![];
    let mut failed = None;
    for piece in pieces {
        match image_pixels(doc, &piece.image) {
            Ok(pixels) => read.push((pixels, piece)),
            Err(error) => failed = Some(error),
        }
    }
    if read.is_empty() {
        return Err(failed.unwrap_or_else(|| anyhow::anyhow!("画像を読めない")));
    }
    let [x0, y0, x1, y1] = read
        .iter()
        .map(|(_, p)| bounds(p.matrix))
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
        .unwrap_or([0.0; 4]);
    let (width, height) = (x1 - x0, y1 - y0);
    ensure!(width > 0.0 && height > 0.0, "大きさのない画像");
    // Pixels per point: the finest piece's, within the limits.
    let mut density = read
        .iter()
        .map(|(pixels, p)| {
            let [a, b, c, d, ..] = p.matrix;
            (pixels.width as f64 / a.hypot(b)).max(pixels.height as f64 / c.hypot(d))
        })
        .filter(|d| d.is_finite())
        .fold(0.0, f64::max);
    density = density
        .min(MAX_COMPOSITE_SIDE / width.max(height))
        .min((MAX_COMPOSITE_PIXELS / (width * height)).sqrt());
    ensure!(density > 0.0, "大きさのない画像");
    let columns = ((width * density).round() as usize).max(1);
    let rows = ((height * density).round() as usize).max(1);
    let mut color = vec![255u8; columns * rows * 3];
    let mut alpha = vec![0u8; columns * rows];
    for (pixels, piece) in &read {
        let [a, b, c, d, e, f] = piece.matrix;
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            continue;
        }
        let [px0, py0, px1, py1] = bounds(piece.matrix);
        let column = |x: f64| (((x - x0) * density).max(0.0) as usize).min(columns);
        let row = |y: f64| (((y1 - y) * density).max(0.0) as usize).min(rows);
        for j in row(py1)..row(py0).saturating_add(1).min(rows) {
            for i in column(px0)..column(px1).saturating_add(1).min(columns) {
                // The point of the page the pixel's centre is, in the unit
                // square of the piece, whose top row is at 1.
                let x = x0 + (i as f64 + 0.5) / density - e;
                let y = y1 - (j as f64 + 0.5) / density - f;
                let u = (d * x - c * y) / det;
                let v = (a * y - b * x) / det;
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let sx = ((u * pixels.width as f64) as usize).min(pixels.width - 1);
                let sy = (((1.0 - v) * pixels.height as f64) as usize).min(pixels.height - 1);
                let at = sy * pixels.width + sx;
                let sample = &pixels.data[at * pixels.channels..(at + 1) * pixels.channels];
                let (rgb, opacity) = if piece.mask {
                    ([0, 0, 0], 255 - sample[0])
                } else {
                    let rgb = if pixels.channels == 1 {
                        [sample[0]; 3]
                    } else {
                        [sample[0], sample[1], sample[2]]
                    };
                    (rgb, pixels.alpha.as_ref().map_or(255, |alpha| alpha[at]))
                };
                let out = j * columns + i;
                let (top, under) = (u32::from(opacity), u32::from(alpha[out]));
                // The piece over what is drawn below it.
                let total = top * 255 + under * (255 - top);
                if total == 0 {
                    continue;
                }
                for (k, channel) in rgb.iter().enumerate() {
                    let below = u32::from(color[out * 3 + k]);
                    color[out * 3 + k] = ((u32::from(*channel) * top * 255
                        + below * under * (255 - top))
                        / total) as u8;
                }
                alpha[out] = (total / 255) as u8;
            }
        }
    }
    let gray = color
        .as_chunks::<3>()
        .0
        .iter()
        .all(|[r, g, b]| r == g && g == b);
    let mut composed = if gray {
        Pixels::new(columns, rows, 1, color.into_iter().step_by(3).collect())
    } else {
        Pixels::new(columns, rows, 3, color)
    };
    composed.alpha = Some(alpha);
    Ok(composed)
}

/// The pixels of JPEG data, as the DCT filter gives its samples to the PDF:
/// as the image stores them, a YCCK image's made CMYK, CMYK samples taken as
/// they are whether or not Adobe software stored them inverted (as PDF
/// readers take them: a PDF inverts those with its Decode array); then
/// mapped by the image's Decode array and read in its color space.
fn jpeg_pixels(doc: &Pdf, dict: &Dictionary, data: &[u8]) -> Result<Pixels> {
    use zune_jpeg::{
        JpegDecoder,
        zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions},
    };
    let unreadable =
        |e: zune_jpeg::errors::DecodeErrors| anyhow::anyhow!("JPEGの画像を読めない（{e}）");
    let options = DecoderOptions::default()
        .set_max_width(1 << 16)
        .set_max_height(1 << 16);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
    decoder.decode_headers().map_err(unreadable)?;
    let (width, height) = decoder.dimensions().context("JPEGの画像を読めない")?;
    let (width, height) = (width, height);
    ensure!(
        (width as u64) * (height as u64) <= MAX_PIXELS,
        "画素数が多すぎる画像"
    );
    let input = decoder.input_colorspace().context("JPEGの画像を読めない")?;
    // Samples as the image stores them, its colors not converted.
    let stored = match input {
        ColorSpace::Luma => ColorSpace::Luma,
        ColorSpace::CMYK => ColorSpace::CMYK,
        ColorSpace::YCCK => ColorSpace::YCCK,
        _ => ColorSpace::RGB,
    };
    decoder.set_options(options.jpeg_set_out_colorspace(stored));
    let mut samples = decoder.decode().map_err(unreadable)?;
    let components = stored.num_components();
    if input == ColorSpace::YCCK {
        for pixel in samples.as_chunks_mut::<4>().0 {
            let (y, cb, cr) = (
                f64::from(pixel[0]),
                f64::from(pixel[1]) - 128.0,
                f64::from(pixel[2]) - 128.0,
            );
            let rgb = [
                y + 1.402 * cr,
                y - 0.344_136 * cb - 0.714_136 * cr,
                y + 1.772 * cb,
            ];
            for (sample, value) in pixel.iter_mut().zip(rgb) {
                *sample = 255 - value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    // The Decode array maps each component's range.
    if let Some(decode) = dict.get(b"Decode").ok().and_then(|d| numbers(doc, d))
        && decode.len() >= 2 * components
    {
        let maps: Vec<[u8; 256]> = (0..components)
            .map(|c| {
                let (from, to) = (decode[2 * c], decode[2 * c + 1]);
                std::array::from_fn(|s| {
                    ((from + (to - from) * s as f64 / 255.0) * 255.0)
                        .round()
                        .clamp(0.0, 255.0) as u8
                })
            })
            .collect();
        for pixel in samples.chunks_exact_mut(components) {
            for (sample, map) in pixel.iter_mut().zip(&maps) {
                *sample = map[usize::from(*sample)];
            }
        }
    }
    let space = dict
        .get(b"ColorSpace")
        .ok()
        .and_then(|s| Space::read(doc, s, 0).ok())
        .filter(|s| s.components() == components && !matches!(s, Space::Indexed(..)))
        .unwrap_or(match components {
            1 => Space::Gray,
            4 => Space::Cmyk,
            _ => Space::Rgb,
        });
    let mut out = Vec::with_capacity(width * height * space.channels());
    for pixel in samples.chunks_exact(components).take(width * height) {
        space.write(pixel, &mut out);
    }
    ensure!(
        out.len() == width * height * space.channels(),
        "JPEGの画像を読めない"
    );
    Ok(Pixels::new(width, height, space.channels(), out))
}

/// Whether an image's Decode array maps each component as it is, from 0 to 1.
fn decode_kept(doc: &Pdf, dict: &Dictionary) -> bool {
    dict.get(b"Decode")
        .ok()
        .and_then(|d| numbers(doc, d))
        .is_none_or(|decode| {
            decode
                .chunks(2)
                .all(|pair| pair.len() == 2 && pair[0] == 0.0 && pair[1] == 1.0)
        })
}

/// An image decoded: JPEG kept as its file, or its pixels.
enum Decoded {
    Jpeg(Vec<u8>),
    Pixels(Pixels),
}

/// The pixels of the image of `dict` (its references followed) whose data
/// is `content`, or its JPEG file.
fn decoded(doc: &Pdf, dict: &Dictionary, content: &[u8]) -> Result<Decoded> {
    let filters: Vec<&[u8]> = match dict.get(b"Filter") {
        Ok(Object::Name(name)) => vec![name.as_slice()],
        Ok(Object::Array(names)) => names
            .iter()
            .map(|n| resolved(doc, n).as_name())
            .collect::<Result<_, _>>()
            .context("フィルターを読めない画像")?,
        Ok(_) => bail!("フィルターを読めない画像"),
        Err(_) => vec![],
    };
    // The parameters of each filter: one dictionary for one filter, or an
    // array with an entry (a dictionary or null) for each.
    let params: Vec<Option<&Dictionary>> = match dict.get(b"DecodeParms").map(|p| resolved(doc, p))
    {
        Ok(Object::Array(entries)) => (0..filters.len())
            .map(|i| entries.get(i).and_then(|e| dictionary(doc, e)))
            .collect(),
        Ok(object) => (0..filters.len())
            .map(|i| (i == 0).then(|| dictionary(doc, object)).flatten())
            .collect(),
        Err(_) => vec![None; filters.len()],
    };
    let codec = filters.last().copied().filter(|f| {
        matches!(
            *f,
            b"DCTDecode" | b"DCT" | b"JPXDecode" | b"CCITTFaxDecode" | b"CCF" | b"JBIG2Decode"
        )
    });
    let general = filters.len() - usize::from(codec.is_some());
    let data =
        unfilter(content, &filters[..general], &params[..general]).context("圧縮を解けない画像")?;
    let inverted = inverted(doc, dict);
    Ok(match codec {
        // A JPEG is kept as its file, which shows what the page draws
        // unless the image's Decode array maps its samples otherwise.
        Some(b"DCTDecode" | b"DCT") => {
            let data = jpeg(data);
            if decode_kept(doc, dict) {
                Decoded::Jpeg(data)
            } else {
                match jpeg_pixels(doc, dict, &data) {
                    Ok(pixels) => Decoded::Pixels(pixels),
                    Err(_) => Decoded::Jpeg(data),
                }
            }
        }
        Some(b"JPXDecode") => {
            // The alpha of the codestream is the image's when SMaskInData says so;
            // a writer that leaves it out still means it when there is no SMask.
            let alpha =
                integer(dict, b"SMaskInData").map_or(dict.get(b"SMask").is_err(), |n| n != 0);
            Decoded::Pixels(jpx_pixels(&data, alpha)?)
        }
        Some(b"CCITTFaxDecode" | b"CCF") => {
            Decoded::Pixels(ccitt_pixels(dict, params[general], &data, inverted)?)
        }
        Some(_) => Decoded::Pixels(jbig2_pixels(doc, params[general], &data, inverted)?),
        None => Decoded::Pixels(samples_pixels(doc, dict, data, inverted)?),
    })
}

/// A JPEG file as an image viewer opens it: from its start of image, which
/// some writers put bytes before, and closed with an end of image when the
/// data stops short of one, so the rows it holds are shown.
fn jpeg(mut data: Vec<u8>) -> Vec<u8> {
    if let Some(start) = data.windows(3).position(|w| w == b"\xff\xd8\xff")
        && start > 0
    {
        data.drain(..start);
    }
    if data.starts_with(b"\xff\xd8") && !data.windows(2).any(|w| w == b"\xff\xd9") {
        data.extend_from_slice(b"\xff\xd9");
    }
    data
}

/// Gives `pixels` the alpha the soft mask or mask of the image of `dict`
/// gives it, as large as they are; none when it has neither or it cannot be
/// read. Colors a soft mask's Matte premultiplied are taken back.
fn masked(doc: &Pdf, dict: &Dictionary, pixels: &mut Pixels) {
    let image = |key: &[u8]| {
        let stream = resolved(doc, dict.get(key).ok()?).as_stream().ok()?;
        let mask = flat(doc, &stream.dict);
        match decoded(doc, &mask, &stream.content).ok()? {
            Decoded::Pixels(gray) => Some((gray, mask)),
            Decoded::Jpeg(_) => None,
        }
    };
    if let Some((gray, mask)) = image(b"SMask") {
        let alpha = gray.resized_first(pixels.width, pixels.height);
        let matte = mask.get(b"Matte").ok().and_then(|m| numbers(doc, m));
        if let Some(matte) = matte.filter(|m| m.len() == pixels.channels) {
            let matte: Vec<f64> = matte.iter().map(|m| m.clamp(0.0, 1.0) * 255.0).collect();
            for (color, a) in pixels.data.chunks_exact_mut(pixels.channels).zip(&alpha) {
                let a = f64::from(*a) / 255.0;
                for (c, m) in color.iter_mut().zip(&matte) {
                    if a > 0.0 {
                        *c = (m + (f64::from(*c) - m) / a).clamp(0.0, 255.0).round() as u8;
                    }
                }
            }
        }
        pixels.alpha = Some(alpha);
        return;
    }
    // A stencil mask paints (leaves shown) where its samples are 0.
    if let Some((stencil, _)) = image(b"Mask") {
        let mut alpha = stencil.resized_first(pixels.width, pixels.height);
        alpha.iter_mut().for_each(|a| *a = 255 - *a);
        pixels.alpha = Some(alpha);
    }
}

/// `content` decoded by `filters`, one at a time, each with its parameters.
fn unfilter(content: &[u8], filters: &[&[u8]], params: &[Option<&Dictionary>]) -> Result<Vec<u8>> {
    let mut data = content.to_vec();
    for (filter, params) in filters.iter().zip(params) {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(filter.to_vec()));
        if let Some(params) = params {
            dict.set("DecodeParms", Object::Dictionary((*params).clone()));
        }
        data = Stream::new(dict, data).decompressed_content_with_limit(MAX_SAMPLES)?;
    }
    Ok(data)
}

/// Whether the image's Decode array maps its first component from 1 to 0,
/// drawing a one-component image (or an image mask) inverted.
fn inverted(doc: &Pdf, dict: &Dictionary) -> bool {
    let Ok(Object::Array(decode)) = dict.get(b"Decode").map(|d| resolved(doc, d)) else {
        return false;
    };
    let number = |i: usize| decode.get(i).and_then(|n| n.as_float().ok());
    matches!((number(0), number(1)), (Some(from), Some(to)) if from > to)
}

/// Bi-level rows as 8-bit gray, black pixels `black` and white ones the other.
struct Bilevel {
    width: usize,
    black: u8,
    pixels: Vec<u8>,
    /// Where the current row starts, for the JBIG2 decoder pushing pixels.
    row: usize,
}

impl Bilevel {
    fn new(width: u32, black: u8) -> Self {
        Self {
            width: width as usize,
            black,
            pixels: vec![],
            row: 0,
        }
    }

    fn color(&self, black: bool) -> u8 {
        if black { self.black } else { 255 - self.black }
    }

    /// The pixels, `height` rows high, rows not decoded left white.
    fn pixels(mut self, height: usize) -> Result<Pixels> {
        ensure!(!self.pixels.is_empty(), "画素を読めない画像");
        let white = self.color(false);
        self.pixels.resize(self.width * height, white);
        Ok(Pixels::new(self.width, height, 1, self.pixels))
    }
}

impl hayro_ccitt::Decoder for Bilevel {
    fn push_row(&mut self, changes: &[u32]) {
        let start = self.pixels.len();
        let white = self.color(false);
        self.pixels.resize(start + self.width, white);
        let black = self.color(true);
        let row = &mut self.pixels[start..];
        for run in changes.chunks(2) {
            let from = (run[0] as usize).min(row.len());
            let to = run
                .get(1)
                .map_or(row.len(), |to| (*to as usize).min(row.len()));
            row[from..to].fill(black);
        }
    }
}

impl hayro_jbig2::Decoder for Bilevel {
    fn push_pixel(&mut self, black: bool) {
        let color = self.color(black);
        self.pixels.push(color);
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        let color = self.color(black);
        self.pixels
            .extend(std::iter::repeat_n(color, chunk_count as usize * 8));
    }

    fn next_line(&mut self) {
        // A row holds the page's width; chunks of 8 can run past it.
        self.row += self.width;
        let white = self.color(false);
        self.pixels.resize(self.row, white);
    }
}

/// CCITT fax data as gray pixels. The pixels the data codes as black are
/// black when the PDF draws them dark.
fn ccitt_pixels(
    dict: &Dictionary,
    params: Option<&Dictionary>,
    data: &[u8],
    inverted: bool,
) -> Result<Pixels> {
    use hayro_ccitt::{DecodeSettings, DecoderContext, EncodingMode};
    let param = |key: &[u8]| params.and_then(|p| integer(p, key));
    let flag = |key: &[u8], default: bool| params.and_then(|p| boolean(p, key)).unwrap_or(default);
    let k = param(b"K").unwrap_or(0);
    let columns = param(b"Columns")
        .or_else(|| integer(dict, b"Width"))
        .unwrap_or(1728);
    let rows = param(b"Rows")
        .filter(|rows| *rows > 0)
        .or_else(|| integer(dict, b"Height"))
        .context("高さのないCCITTの画像")?;
    let columns = u32::try_from(columns).context("幅が負のCCITTの画像")?;
    let rows = u32::try_from(rows).context("高さが負のCCITTの画像")?;
    ensure!(
        u64::from(columns) * u64::from(rows) <= MAX_PIXELS,
        "画素数が多すぎる画像"
    );
    // A coded black pixel is the sample BlackIs1 gives it, which the Decode
    // array maps to dark when it does not invert that sample (an image mask
    // paints sample 0; a gray image draws sample 0 black).
    let dark = flag(b"BlackIs1", false) == inverted;
    let mut context = DecoderContext::new(DecodeSettings {
        columns,
        rows,
        end_of_block: flag(b"EndOfBlock", true),
        end_of_line: flag(b"EndOfLine", false),
        rows_are_byte_aligned: flag(b"EncodedByteAlign", false),
        encoding: match k {
            ..0 => EncodingMode::Group4,
            0 => EncodingMode::Group3_1D,
            _ => EncodingMode::Group3_2D,
        },
        strict: false,
    });
    let mut image = Bilevel::new(columns, if dark { 0 } else { 255 });
    // Rows decoded before damaged data are drawn, as PDF readers draw them.
    let decoded = hayro_ccitt::decode(data, &mut image, &mut context);
    if image.pixels.is_empty() {
        decoded.map_err(|e| anyhow::anyhow!("CCITTの画像を読めない（{e:?}）"))?;
    }
    image.pixels(rows as usize)
}

/// JBIG2 data, with the global segments its parameters name, as gray
/// pixels. JBIG2 codes black as 1, which the filter gives as sample 0.
fn jbig2_pixels(
    doc: &Pdf,
    params: Option<&Dictionary>,
    data: &[u8],
    inverted: bool,
) -> Result<Pixels> {
    use hayro_jbig2::{DecodeSettings, Globals, Page};
    let settings = DecodeSettings::default();
    let globals = match params.and_then(|p| p.get(b"JBIG2Globals").ok()) {
        Some(globals) => {
            let stream = resolved(doc, globals)
                .as_stream()
                .context("JBIG2の共通部分を読めない画像")?;
            let bytes = stream
                .decompressed_content_with_limit(MAX_SAMPLES)
                .context("JBIG2の共通部分を読めない画像")?;
            Some(
                Globals::new(&bytes, &settings)
                    .map_err(|e| anyhow::anyhow!("JBIG2の共通部分を読めない画像（{e}）"))?,
            )
        }
        None => None,
    };
    let page = Page::new_embedded(data, globals.as_ref(), &settings)
        .map_err(|e| anyhow::anyhow!("JBIG2の画像を読めない（{e}）"))?;
    let (width, height) = (page.width(), page.height());
    ensure!(
        u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "画素数が多すぎる画像"
    );
    let mut image = Bilevel::new(width, if inverted { 255 } else { 0 });
    page.decode(&mut image)
        .map_err(|e| anyhow::anyhow!("JBIG2の画像を読めない（{e}）"))?;
    image.pixels(height as usize)
}

/// JPEG 2000 data as 8-bit gray or RGB pixels, with its alpha channel when
/// it has one and `alpha` says to use it.
fn jpx_pixels(data: &[u8], alpha: bool) -> Result<Pixels> {
    use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext, Image};
    let image = Image::new(data, &DecodeSettings::default())
        .map_err(|e| anyhow::anyhow!("JPEG 2000の画像を読めない（{e}）"))?;
    let (width, height) = (image.width(), image.height());
    ensure!(
        u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "画素数が多すぎる画像"
    );
    let space = match image.color_space() {
        ColorSpace::Gray => Space::Gray,
        ColorSpace::RGB => Space::Rgb,
        ColorSpace::CMYK => Space::Cmyk,
        ColorSpace::Icc { num_channels, .. } | ColorSpace::Unknown { num_channels } => {
            match num_channels {
                1 => Space::Gray,
                3 => Space::Rgb,
                4 => Space::Cmyk,
                n => bail!("色の成分が{n}個のJPEG 2000の画像"),
            }
        }
    };
    let mut context = DecoderContext::default();
    let decoded = image
        .decode(&mut context)
        .map_err(|e| anyhow::anyhow!("JPEG 2000の画像を読めない（{e}）"))?;
    let samples = decoded.data_u8();
    let components = space.components();
    let stride = components + usize::from(image.has_alpha());
    let pixels = (width as usize) * (height as usize);
    ensure!(
        samples.len() >= pixels * stride,
        "JPEG 2000の画像を読めない"
    );
    let mut out = Vec::with_capacity(pixels * space.channels());
    let mut opacity = Vec::with_capacity(if image.has_alpha() { pixels } else { 0 });
    for pixel in samples.chunks_exact(stride).take(pixels) {
        space.write(&pixel[..components], &mut out);
        if image.has_alpha() {
            opacity.push(pixel[components]);
        }
    }
    let mut decoded = Pixels::new(width as usize, height as usize, space.channels(), out);
    if alpha && !opacity.is_empty() {
        decoded.alpha = Some(opacity);
    }
    Ok(decoded)
}

/// A color space an image's samples are read in.
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// One colorant, as a Separation names it: 1 is full ink, drawn dark.
    Ink,
    /// Indexes into a palette of colors of the base space.
    Indexed(Box<Space>, Vec<u8>),
    /// CIE L*a*b*, with the range of a* and b*. Its white point is taken as
    /// D65's, which shows the colors near enough to read.
    Lab([f64; 4]),
    /// Colorants by name (DeviceN): the process colors are those of CMYK,
    /// every other one an ink drawn dark like black; None paints nothing.
    Inks(Vec<Ink>),
}

/// A DeviceN colorant.
#[derive(Clone, Copy)]
enum Ink {
    Cyan,
    Magenta,
    Yellow,
    Black,
    Spot,
    None,
}

impl Space {
    fn components(&self) -> usize {
        match self {
            Self::Gray | Self::Ink | Self::Indexed(..) => 1,
            Self::Rgb | Self::Lab(..) => 3,
            Self::Cmyk => 4,
            Self::Inks(inks) => inks.len(),
        }
    }

    fn read(doc: &Pdf, object: &Object, depth: usize) -> Result<Self> {
        ensure!(depth < 4, "色空間を読めない画像");
        Ok(match resolved(doc, object) {
            Object::Name(name) => match name.as_slice() {
                b"DeviceGray" | b"CalGray" | b"G" => Self::Gray,
                b"DeviceRGB" | b"CalRGB" | b"RGB" => Self::Rgb,
                b"DeviceCMYK" | b"CMYK" => Self::Cmyk,
                other => bail!("色空間{}の画像", String::from_utf8_lossy(other)),
            },
            Object::Array(entries) => {
                let family = entries
                    .first()
                    .map(|f| resolved(doc, f))
                    .and_then(|f| f.as_name().ok())
                    .context("色空間を読めない画像")?;
                match family {
                    b"DeviceGray" | b"CalGray" => Self::Gray,
                    b"DeviceRGB" | b"CalRGB" => Self::Rgb,
                    b"DeviceCMYK" => Self::Cmyk,
                    b"ICCBased" => {
                        let profile = entries
                            .get(1)
                            .and_then(|p| resolved(doc, p).as_stream().ok())
                            .context("ICCプロファイルを読めない画像")?;
                        match integer(&profile.dict, b"N") {
                            Some(1) => Self::Gray,
                            Some(3) => Self::Rgb,
                            Some(4) => Self::Cmyk,
                            _ => bail!("色空間ICCBasedの成分数を読めない画像"),
                        }
                    }
                    b"Separation" => Self::Ink,
                    b"DeviceN" => {
                        let names = entries
                            .get(1)
                            .and_then(|names| resolved(doc, names).as_array().ok())
                            .context("色材の名前のない画像")?;
                        ensure!(
                            (1..=32).contains(&names.len()),
                            "色材が{}個の画像",
                            names.len()
                        );
                        let inks: Vec<Ink> = names
                            .iter()
                            .map(|name| match resolved(doc, name).as_name() {
                                Ok(b"Cyan") => Ink::Cyan,
                                Ok(b"Magenta") => Ink::Magenta,
                                Ok(b"Yellow") => Ink::Yellow,
                                Ok(b"Black") => Ink::Black,
                                Ok(b"None") => Ink::None,
                                _ => Ink::Spot,
                            })
                            .collect();
                        if inks.len() == 1 {
                            Self::Ink
                        } else {
                            Self::Inks(inks)
                        }
                    }
                    b"Lab" => {
                        let params = entries.get(1).and_then(|p| dictionary(doc, p));
                        let get = |key: &[u8]| {
                            params
                                .and_then(|p| p.get(key).ok())
                                .and_then(|n| numbers(doc, n))
                        };
                        let range = get(b"Range")
                            .and_then(|r| <[f64; 4]>::try_from(r).ok())
                            .unwrap_or([-100.0, 100.0, -100.0, 100.0]);
                        Self::Lab(range)
                    }
                    b"Indexed" | b"I" => {
                        let base = Self::read(
                            doc,
                            entries.get(1).context("基底の色空間のない画像")?,
                            depth + 1,
                        )?;
                        ensure!(!matches!(base, Self::Indexed(..)), "色空間を読めない画像");
                        let palette = match entries.get(3).map(|l| resolved(doc, l)) {
                            Some(Object::String(bytes, _)) => bytes.clone(),
                            Some(Object::Stream(stream)) => {
                                stream.decompressed_content_with_limit(MAX_SAMPLES)?
                            }
                            _ => bail!("色の表のない画像"),
                        };
                        Self::Indexed(Box::new(base), palette)
                    }
                    other => bail!("色空間{}の画像", String::from_utf8_lossy(other)),
                }
            }
            _ => bail!("色空間を読めない画像"),
        })
    }

    /// The channels (1 gray or 3 RGB) a sample becomes.
    fn channels(&self) -> usize {
        match self {
            Self::Gray | Self::Ink => 1,
            Self::Indexed(base, _) => base.channels(),
            Self::Rgb | Self::Cmyk | Self::Lab(..) | Self::Inks(_) => 3,
        }
    }

    /// Writes the 8-bit color of `components` (each 0 to 255) to `out`.
    fn write(&self, components: &[u8], out: &mut Vec<u8>) {
        match self {
            Self::Gray => out.push(components[0]),
            Self::Ink => out.push(255 - components[0]),
            Self::Rgb => out.extend_from_slice(&components[..3]),
            Self::Cmyk => {
                let k = 255 - u32::from(components[3]);
                out.extend(
                    components[..3]
                        .iter()
                        .map(|c| ((255 - u32::from(*c)) * k / 255) as u8),
                );
            }
            Self::Indexed(base, palette) => {
                let width = base.components();
                let start = usize::from(components[0]) * width;
                match palette.get(start..start + width) {
                    Some(color) => base.write(color, out),
                    None => out.extend(std::iter::repeat_n(0, base.channels())),
                }
            }
            Self::Lab(range) => {
                let l = f64::from(components[0]) * 100.0 / 255.0;
                let a = range[0] + f64::from(components[1]) * (range[1] - range[0]) / 255.0;
                let b = range[2] + f64::from(components[2]) * (range[3] - range[2]) / 255.0;
                let f = |t: f64| {
                    if t > 6.0 / 29.0 {
                        t * t * t
                    } else {
                        3.0 * (6.0f64 / 29.0).powi(2) * (t - 4.0 / 29.0)
                    }
                };
                let y = (l + 16.0) / 116.0;
                let (x, y, z) = (f(y + a / 500.0) * 0.9505, f(y), f(y - b / 200.0) * 1.089);
                let linear = [
                    3.2406 * x - 1.5372 * y - 0.4986 * z,
                    -0.9689 * x + 1.8758 * y + 0.0415 * z,
                    0.0557 * x - 0.2040 * y + 1.0570 * z,
                ];
                out.extend(linear.map(|c| {
                    let c = c.clamp(0.0, 1.0);
                    let c = if c <= 0.0031308 {
                        12.92 * c
                    } else {
                        1.055 * c.powf(1.0 / 2.4) - 0.055
                    };
                    (c * 255.0).round() as u8
                }));
            }
            Self::Inks(inks) => {
                let mut cmyk = [0u32; 4];
                for (ink, value) in inks.iter().zip(components) {
                    let channel = match ink {
                        Ink::Cyan => 0,
                        Ink::Magenta => 1,
                        Ink::Yellow => 2,
                        Ink::Black | Ink::Spot => 3,
                        Ink::None => continue,
                    };
                    cmyk[channel] = (cmyk[channel] + u32::from(*value)).min(255);
                }
                Self::Cmyk.write(&cmyk.map(|c| c as u8), out);
            }
        }
    }
}

/// An image's samples as 8-bit gray or RGB pixels, transparent where a
/// color key mask (a Mask array of sample ranges) keys them out.
fn samples_pixels(
    doc: &Pdf,
    dict: &Dictionary,
    mut data: Vec<u8>,
    inverted: bool,
) -> Result<Pixels> {
    let width = integer(dict, b"Width").context("幅のない画像")?;
    let height = integer(dict, b"Height").context("高さのない画像")?;
    ensure!(width > 0 && height > 0, "幅か高さが0の画像");
    let (width, height) = (width as u64, height as u64);
    ensure!(width * height <= MAX_PIXELS, "画素数が多すぎる画像");
    let mask = boolean(dict, b"ImageMask").unwrap_or(false);
    let bits = if mask {
        1
    } else {
        integer(dict, b"BitsPerComponent").context("成分のビット数のない画像")?
    };
    ensure!(
        matches!(bits, 1 | 2 | 4 | 8 | 16),
        "成分が{bits}ビットの画像"
    );
    let space = if mask {
        Space::Gray
    } else {
        Space::read(doc, dict.get(b"ColorSpace").context("色空間のない画像")?, 0)?
    };
    let indexed = matches!(space, Space::Indexed(..));
    let components = space.components();
    let channels = space.channels();
    let bits = bits as usize;
    let row = (width as usize * components * bits).div_ceil(8);
    let (width, height) = (width as usize, height as usize);
    ensure!(row * height <= MAX_SAMPLES, "画素数が多すぎる画像");
    // Data cut short is drawn as far as it goes, as PDF readers draw it.
    data.resize(row * height, 0);
    let max = (1u32 << bits) - 1;
    // The color key: a pixel each of whose samples is in its range is not
    // painted. A 16-bit sample is compared by its high byte.
    let key: Option<Vec<u32>> = match dict.get(b"Mask").map(|m| resolved(doc, m)) {
        Ok(Object::Array(ranges)) if !mask && ranges.len() == 2 * components => ranges
            .iter()
            .map(|r| {
                resolved(doc, r)
                    .as_i64()
                    .ok()
                    .map(|n| (n.max(0) as u32) >> (bits.saturating_sub(8) as u32))
            })
            .collect(),
        _ => None,
    };
    let mut pixels = Vec::with_capacity(width * height * channels);
    let mut alpha = Vec::with_capacity(if key.is_some() { width * height } else { 0 });
    let mut color = vec![0u8; components];
    for line in data.chunks_exact(row) {
        let mut bit = 0;
        for _ in 0..width {
            let mut keyed = key.is_some();
            for (c, component) in color.iter_mut().enumerate() {
                let sample = match bits {
                    8 => u32::from(line[bit / 8]),
                    16 => u32::from(line[bit / 8]),
                    _ => (u32::from(line[bit / 8]) >> (8 - bits - bit % 8)) & max,
                };
                if let Some(key) = &key {
                    keyed &= (key[2 * c]..=key[2 * c + 1]).contains(&sample);
                }
                bit += bits;
                *component = if indexed || bits >= 8 {
                    sample as u8
                } else {
                    (sample * 255 / max) as u8
                };
                // An image mask paints sample 0 (black ink); gray draws 0 black.
                if inverted && components == 1 && !indexed {
                    *component = 255 - *component;
                }
            }
            space.write(&color, &mut pixels);
            if key.is_some() {
                alpha.push(if keyed { 0 } else { 255 });
            }
        }
    }
    let mut decoded = Pixels::new(width, height, channels, pixels);
    if key.is_some() {
        decoded.alpha = Some(alpha);
    }
    Ok(decoded)
}

/// Decoded pixels: 8-bit gray (one channel) or RGB (three), and the opacity
/// of each when the image is not all opaque.
struct Pixels {
    width: usize,
    height: usize,
    channels: usize,
    data: Vec<u8>,
    alpha: Option<Vec<u8>>,
}

impl Pixels {
    fn new(width: usize, height: usize, channels: usize, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            channels,
            data,
            alpha: None,
        }
    }

    /// The first channel, scaled to `width` by `height` by taking the
    /// nearest pixel: a soft mask may be larger or smaller than its image.
    fn resized_first(&self, width: usize, height: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            let from = (y * self.height / height.max(1)).min(self.height.saturating_sub(1));
            for x in 0..width {
                let column = (x * self.width / width.max(1)).min(self.width.saturating_sub(1));
                out.push(
                    self.data
                        .get((from * self.width + column) * self.channels)
                        .copied()
                        .unwrap_or(255),
                );
            }
        }
        out
    }

    /// The pixels as a PNG: gray or RGB, with alpha unless all opaque.
    fn png(&self) -> Vec<u8> {
        let alpha = self.alpha.as_ref().filter(|a| a.iter().any(|a| *a < 255));
        let Some(alpha) = alpha else {
            return png(
                self.width as u32,
                self.height as u32,
                self.channels,
                &self.data,
            );
        };
        let mut pixels = Vec::with_capacity(self.data.len() + alpha.len());
        for (color, alpha) in self.data.chunks_exact(self.channels).zip(alpha) {
            pixels.extend_from_slice(color);
            pixels.push(*alpha);
        }
        png(
            self.width as u32,
            self.height as u32,
            self.channels + 1,
            &pixels,
        )
    }
}

/// 8-bit gray (one channel), gray and alpha (two), RGB (three) or RGBA
/// (four) pixels as a PNG.
fn png(width: u32, height: u32, channels: usize, pixels: &[u8]) -> Vec<u8> {
    use flate2::{Compression, Crc, write::ZlibEncoder};
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(kind);
        out.extend(data);
        let mut crc = Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend(crc.sum().to_be_bytes());
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = width.to_be_bytes().to_vec();
    header.extend(height.to_be_bytes());
    let color_type = match channels {
        1 => 0,
        2 => 4,
        3 => 2,
        _ => 6,
    };
    header.extend([8, color_type, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    let mut encoder = ZlibEncoder::new(vec![], Compression::default());
    for line in pixels.chunks(width as usize * channels) {
        // Writing to a vector does not fail.
        encoder.write_all(&[0]).unwrap();
        encoder.write_all(line).unwrap();
    }
    chunk(&mut out, b"IDAT", &encoder.finish().unwrap());
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{content::Operation, dictionary};

    /// A one-page PDF drawing `image` once directly and once through a form.
    fn drawing(image: Stream) -> Pdf {
        let mut doc = Pdf::with_version("1.5");
        let image = doc.add_object(image);
        let form_content = Content {
            operations: vec![Operation::new("Do", vec!["Im0".into()])],
        };
        let form = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
                "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
            },
            form_content.encode().unwrap(),
        ));
        let content = Content {
            operations: vec![
                Operation::new("Do", vec!["Im1".into()]),
                Operation::new("Do", vec!["Fm1".into()]),
            ],
        };
        let content = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image, "Fm1" => form } },
        });
        doc.objects.insert(
            pages,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        doc
    }

    fn png_pixels(png: &[u8]) -> (u8, Vec<u8>) {
        use std::io::Read;
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let color_type = png[25];
        let length = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let mut pixels = vec![];
        flate2::read::ZlibDecoder::new(&png[41..41 + length])
            .read_to_end(&mut pixels)
            .unwrap();
        (color_type, pixels)
    }

    #[test]
    fn an_image_is_placed_where_the_page_and_its_forms_map_its_unit_square() {
        let mut doc = drawing(Stream::new(
            dictionary! { "Subtype" => "Image", "Width" => 1, "Height" => 1, "ImageMask" => true },
            vec![0],
        ));
        // The page scales the image to 200 by 100 at (10, 20) and then draws
        // the form, whose matrix moves what it draws by (5, 0).
        let page = doc.get_pages()[&1];
        let contents = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        let content = b"q 200 0 0 100 10 20 cm /Im1 Do Q /Fm1 Do".to_vec();
        doc.objects.insert(
            contents,
            Object::Stream(Stream::new(dictionary! {}, content)),
        );
        let resources = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .clone();
        let form = resources
            .get(b"XObject")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Fm1")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_object_mut(form)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .dict
            .set(
                "Matrix",
                vec![1.into(), 0.into(), 0.into(), 1.into(), 5.into(), 0.into()],
            );
        let Placements {
            images: order,
            pages,
        } = placements(&doc, 1 << 20).unwrap();
        assert_eq!(order.len(), 1);
        let placed: Vec<_> = pages[0]
            .1
            .iter()
            .map(|p| (p.index, p.matrix.unwrap(), p.mask))
            .collect();
        assert_eq!(
            placed,
            [
                (0, [200.0, 0.0, 0.0, 100.0, 10.0, 20.0], true),
                (0, [1.0, 0.0, 0.0, 1.0, 5.0, 0.0], true),
            ]
        );
    }

    /// A one-page PDF drawing `content` with a color space `CS0`.
    fn inline_page(content: &[u8]) -> Pdf {
        let mut doc = drawing(Stream::new(dictionary! { "Subtype" => "Image" }, vec![]));
        let page = doc.get_pages()[&1];
        let dict = doc.get_dictionary_mut(page).unwrap();
        let contents = dict.get(b"Contents").unwrap().as_reference().unwrap();
        let palette = Object::string_literal(vec![0u8, 0, 0, 255, 255, 255]);
        dict.set(
            "Resources",
            dictionary! { "ColorSpace" => dictionary! {
                "CS0" => vec!["Indexed".into(), "DeviceRGB".into(), 1.into(), palette],
            } },
        );
        doc.objects.insert(
            contents,
            Object::Stream(Stream::new(dictionary! {}, content.to_vec())),
        );
        doc
    }

    #[test]
    fn inline_images_are_read_with_their_abbreviations_and_small_ones_left_out() {
        // A 32 by 32 gray image, drawn twice, and a palette image in a named
        // color space, which lopdf does not read.
        let mut content =
            b"q 100 0 0 100 0 0 cm BI /W 32 /H 32 /CS /G /BPC 8 /F /AHx ID\n".to_vec();
        let hex: String = (0..32 * 32)
            .map(|i| format!("{:02x}", (i % 2) * 255))
            .collect();
        content.extend(hex.as_bytes());
        content.extend(b">\nEI Q ");
        let again = content.clone();
        content.extend(&again);
        content.extend(b"BI /W 32 /H 32 /CS /CS0 /BPC 1 ID\n");
        content.extend(vec![0b1010_1010; 4 * 32]);
        content.extend(b"\nEI ");
        // A bullet too small to hold text.
        content.extend(b"BI /W 4 /H 4 /CS /G /BPC 8 ID\n0123456789abcdef\nEI");
        let doc = inline_page(&content);
        let images = read(&doc, 1 << 20).unwrap();
        assert_eq!(images.files.len(), 2, "{:?}", images.files.keys());
        let (color_type, pixels) = png_pixels(&images.files["image-001.png"]);
        assert_eq!((color_type, &pixels[..4]), (0, &[0, 0, 255, 0][..]));
        let (color_type, pixels) = png_pixels(&images.files["image-002.png"]);
        assert_eq!(
            (color_type, &pixels[..7]),
            (2, &[0, 255, 255, 255, 0, 0, 0][..])
        );
        let pages = placements(&doc, 1 << 20).unwrap().pages;
        let placed: Vec<_> = pages[0]
            .1
            .iter()
            .map(|p| (p.index, p.matrix.unwrap()[0]))
            .collect();
        assert_eq!(placed, [(0, 100.0), (0, 100.0), (1, 1.0)]);
    }

    #[test]
    fn an_image_drawn_directly_and_in_a_form_is_one_file_of_its_page() {
        let image = Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => 2, "Height" => 1,
                "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
            },
            vec![255, 0, 0, 0, 0, 255],
        );
        let images = read(&drawing(image), 1 << 20).unwrap();
        assert_eq!(images.files.len(), 1);
        assert_eq!(images.pages["image-001.png"], [1]);
        assert!(images.note.is_empty());
        let (color_type, pixels) = png_pixels(&images.files["image-001.png"]);
        assert_eq!(color_type, 2);
        assert_eq!(pixels, [0, 255, 0, 0, 0, 0, 255]);
    }

    #[test]
    fn one_bit_masks_indexed_and_cmyk_samples_become_gray_or_rgb() {
        let doc = Pdf::with_version("1.5");
        // An image mask inverted by its Decode array: 1 paints.
        let mask = dictionary! {
            "Subtype" => "Image", "Width" => 3, "Height" => 1, "ImageMask" => true,
            "Decode" => vec![1.into(), 0.into()],
        };
        let png = samples_pixels(&doc, &mask, vec![0b1010_0000], inverted(&doc, &mask))
            .unwrap()
            .png();
        assert_eq!(png_pixels(&png), (0, vec![0, 0, 255, 0]));
        let indexed = dictionary! {
            "Width" => 2, "Height" => 1, "BitsPerComponent" => 8,
            "ColorSpace" => vec!["Indexed".into(), "DeviceGray".into(), 1.into(), Object::string_literal(vec![10u8, 20])],
        };
        let png = samples_pixels(&doc, &indexed, vec![1, 0], false)
            .unwrap()
            .png();
        assert_eq!(png_pixels(&png), (0, vec![0, 20, 10]));
        let cmyk = dictionary! {
            "Width" => 1, "Height" => 1, "BitsPerComponent" => 8, "ColorSpace" => "DeviceCMYK",
        };
        let png = samples_pixels(&doc, &cmyk, vec![0, 255, 255, 0], false)
            .unwrap()
            .png();
        assert_eq!(png_pixels(&png), (2, vec![0, 255, 0, 0]));
    }

    /// The rows of a 16 by 3 bitmap (black where true), and their Group 4 code.
    fn bitmap() -> (Vec<Vec<bool>>, Vec<u8>) {
        let rows: Vec<Vec<bool>> = vec![
            (0..16).map(|x| x < 4).collect(),
            vec![false; 16],
            (0..16).map(|x| x >= 8).collect(),
        ];
        let mut encoder = fax::encoder::Encoder::new(fax::VecWriter::new());
        for row in &rows {
            let pels = row.iter().map(|black| {
                if *black {
                    fax::Color::Black
                } else {
                    fax::Color::White
                }
            });
            encoder.encode_line(pels, 16).unwrap();
        }
        (rows, encoder.finish().unwrap().finish())
    }

    /// The gray pixels of `rows`, black `black` and white the other.
    fn gray(rows: &[Vec<bool>], black: u8) -> Vec<u8> {
        rows.iter()
            .flat_map(|row| {
                std::iter::once(0).chain(
                    row.iter()
                        .map(move |b| if *b { black } else { 255 - black }),
                )
            })
            .collect()
    }

    /// A JBIG2 segment of page 1 in the embedded organization.
    fn segment(number: u32, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut out = number.to_be_bytes().to_vec();
        out.extend([kind, 0, 1]);
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn ccitt_and_jbig2_are_decoded_black_where_the_pdf_draws_dark() {
        let (rows, code) = bitmap();
        let fax = |black_is_1: bool, decode: Option<Object>| {
            let mut dict = dictionary! {
                "Subtype" => "Image", "Width" => 16, "Height" => 3, "Filter" => "CCITTFaxDecode",
                "BitsPerComponent" => 1, "ColorSpace" => "DeviceGray",
                "DecodeParms" => dictionary! { "K" => -1, "Columns" => 16, "BlackIs1" => black_is_1 },
            };
            if let Some(decode) = decode {
                dict.set("Decode", decode);
            }
            let images = read(&drawing(Stream::new(dict, code.clone())), 1 << 20).unwrap();
            png_pixels(&images.files["image-001.png"])
        };
        assert_eq!(fax(false, None), (0, gray(&rows, 0)));
        // BlackIs1 gives coded black the sample 1, which gray draws white.
        assert_eq!(fax(true, None), (0, gray(&rows, 255)));
        let inverting = Object::Array(vec![1.into(), 0.into()]);
        assert_eq!(fax(true, Some(inverting)), (0, gray(&rows, 0)));

        // The same code as the MMR data of a JBIG2 generic region.
        let mut page = vec![0; 8];
        page[..4].copy_from_slice(&16u32.to_be_bytes());
        page[4..8].copy_from_slice(&3u32.to_be_bytes());
        page.extend([0; 11]);
        let mut region = 16u32.to_be_bytes().to_vec();
        region.extend(3u32.to_be_bytes());
        region.extend([0; 9]);
        region.push(1); // MMR
        region.extend(&code);
        let jbig2 = [
            segment(0, 48, &page),
            segment(1, 38, &region),
            segment(2, 49, &[]),
        ]
        .concat();
        let image = |decode: Option<Object>| {
            let mut dict = dictionary! {
                "Subtype" => "Image", "Width" => 16, "Height" => 3, "Filter" => "JBIG2Decode",
                "BitsPerComponent" => 1, "ColorSpace" => "DeviceGray",
            };
            if let Some(decode) = decode {
                dict.set("Decode", decode);
            }
            let images = read(&drawing(Stream::new(dict, jbig2.clone())), 1 << 20).unwrap();
            assert!(images.note.is_empty(), "{}", images.note);
            png_pixels(&images.files["image-001.png"])
        };
        assert_eq!(image(None), (0, gray(&rows, 0)));
        assert_eq!(
            image(Some(Object::Array(vec![1.into(), 0.into()]))),
            (0, gray(&rows, 255))
        );
    }

    #[test]
    fn jpeg_is_kept_jpeg_2000_is_decoded_and_unreadable_images_are_named() {
        let jpeg = Stream::new(
            dictionary! { "Subtype" => "Image", "Width" => 1, "Height" => 1, "Filter" => "DCTDecode" },
            b"\xff\xd8jpeg".to_vec(),
        );
        let images = read(&drawing(jpeg), 1 << 20).unwrap();
        // Data stopping short of its end of image is closed with one.
        assert_eq!(images.files["image-001.jpg"], b"\xff\xd8jpeg\xff\xd9");
        let jpx = Stream::new(
            dictionary! { "Subtype" => "Image", "Width" => 4, "Height" => 2, "Filter" => "JPXDecode" },
            include_bytes!("../../tests/support/pdf/rgb-4x2.jp2").to_vec(),
        );
        let images = read(&drawing(jpx), 1 << 20).unwrap();
        let (color_type, pixels) = png_pixels(&images.files["image-001.png"]);
        assert_eq!(color_type, 2);
        assert_eq!(
            pixels,
            [
                0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, //
                0, 0, 0, 0, 128, 128, 128, 255, 255, 0, 0, 255, 255,
            ]
        );
        let pattern = Stream::new(
            dictionary! {
                "Subtype" => "Image", "Width" => 1, "Height" => 1, "BitsPerComponent" => 8,
                "ColorSpace" => "Pattern",
            },
            vec![0],
        );
        let images = read(&drawing(pattern), 1 << 20).unwrap();
        assert!(images.files.is_empty());
        assert!(
            images.note.contains("1ページ（色空間Patternの画像）"),
            "{}",
            images.note
        );
        let broken = Stream::new(
            dictionary! { "Subtype" => "Image", "Width" => 1, "Height" => 1, "Filter" => "JBIG2Decode" },
            b"jb2".to_vec(),
        );
        let images = read(&drawing(broken), 1 << 20).unwrap();
        assert!(images.files.is_empty());
        assert!(
            images.note.contains("JBIG2の画像を読めない"),
            "{}",
            images.note
        );
    }
    /// The images `doc` gives files, with the page drawing the image of
    /// `image` given the mask streams `masks` under their keys.
    fn masked_image(image: Stream, masks: Vec<(&str, Stream)>) -> ImageAssets {
        let mut doc = drawing(image);
        let page = doc.get_pages()[&1];
        let id = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"XObject")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Im1")
            .unwrap()
            .as_reference()
            .unwrap();
        for (key, mask) in masks {
            let mask = doc.add_object(mask);
            doc.get_object_mut(id)
                .unwrap()
                .as_stream_mut()
                .unwrap()
                .dict
                .set(key, mask);
        }
        read(&doc, 1 << 20).unwrap()
    }

    fn gray_row(width: i64, samples: Vec<u8>) -> Stream {
        Stream::new(
            dictionary! {
                "Subtype" => "Image", "Width" => width, "Height" => 1,
                "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8,
            },
            samples,
        )
    }

    #[test]
    fn soft_masks_masks_and_color_keys_become_the_png_alpha() {
        // Black where the soft mask shows it, transparent where it does not:
        // a transparent picture's text, its hidden colors black too.
        let images = masked_image(
            gray_row(2, vec![0, 0]),
            vec![("SMask", gray_row(2, vec![255, 0]))],
        );
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (4, vec![0, 0, 255, 0, 0])
        );
        // A soft mask half as wide covers the image, each sample two pixels.
        let images = masked_image(
            gray_row(2, vec![9, 9]),
            vec![("SMask", gray_row(1, vec![7]))],
        );
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (4, vec![0, 9, 7, 9, 7])
        );
        // Colors premultiplied against a white matte are taken back.
        let mut matte = gray_row(1, vec![128]);
        matte.dict.set("Matte", vec![1.into()]);
        let images = masked_image(gray_row(1, vec![128]), vec![("SMask", matte)]);
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (4, vec![0, 2, 128])
        );
        // A stencil mask leaves shown where its sample is 0.
        let stencil = Stream::new(
            dictionary! { "Subtype" => "Image", "Width" => 2, "Height" => 1, "ImageMask" => true },
            vec![0b0100_0000],
        );
        let images = masked_image(gray_row(2, vec![10, 200]), vec![("Mask", stencil)]);
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (4, vec![0, 10, 255, 200, 0])
        );
        // A color key keys out the samples in its range.
        let mut keyed = gray_row(2, vec![10, 200]);
        keyed.dict.set("Mask", vec![0.into(), 50.into()]);
        let images = read(&drawing(keyed), 1 << 20).unwrap();
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (4, vec![0, 10, 0, 200, 255])
        );
        // An image all opaque stays without alpha.
        let images = masked_image(
            gray_row(1, vec![5]),
            vec![("SMask", gray_row(1, vec![255]))],
        );
        assert_eq!(png_pixels(&images.files["image-001.png"]), (0, vec![0, 5]));
    }

    #[test]
    fn lab_and_named_colorants_become_rgb() {
        let doc = Pdf::with_version("1.5");
        let lab = dictionary! {
            "Width" => 2, "Height" => 1, "BitsPerComponent" => 8,
            "ColorSpace" => vec!["Lab".into(), dictionary! {
                "WhitePoint" => vec![0.9505.into(), 1.into(), 1.089.into()],
                "Range" => vec![(-128).into(), 127.into(), (-128).into(), 127.into()],
            }.into()],
        };
        // L* 100 and 0 with a* and b* at 0: white and black.
        let (color_type, pixels) = png_pixels(
            &samples_pixels(&doc, &lab, vec![255, 128, 128, 0, 128, 128], false)
                .unwrap()
                .png(),
        );
        assert_eq!(color_type, 2);
        assert!(pixels[1..4].iter().all(|c| *c >= 250), "{pixels:?}");
        assert!(pixels[4..7].iter().all(|c| *c <= 5), "{pixels:?}");
        // Cyan and a spot ink, which is drawn dark like black.
        let inks = dictionary! {
            "Width" => 2, "Height" => 1, "BitsPerComponent" => 8,
            "ColorSpace" => vec!["DeviceN".into(), vec!["Cyan".into(), "Gold".into()].into(), "DeviceCMYK".into(), dictionary! {}.into()],
        };
        let png = samples_pixels(&doc, &inks, vec![255, 0, 0, 255], false)
            .unwrap()
            .png();
        assert_eq!(png_pixels(&png), (2, vec![0, 0, 255, 255, 0, 0, 0]));
    }

    #[test]
    fn jpeg_data_is_opened_from_its_start_and_closed() {
        assert_eq!(
            jpeg(b"\0\r\n\xff\xd8\xff\xe0data".to_vec()),
            b"\xff\xd8\xff\xe0data\xff\xd9"
        );
        assert_eq!(
            jpeg(b"\xff\xd8\xff\xe0data\xff\xd9\0\0".to_vec()),
            b"\xff\xd8\xff\xe0data\xff\xd9\0\0"
        );
    }

    /// A one-page PDF of `content` with `resources`, under a page tree
    /// giving `inherited` as the resources of its pages.
    fn page_of(
        content: &[u8],
        resources: Option<Dictionary>,
        inherited: Dictionary,
        extra: Dictionary,
    ) -> Pdf {
        let mut doc = Pdf::with_version("1.5");
        let contents = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
        let pages = doc.new_object_id();
        let mut page = dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => contents,
            "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()],
        };
        if let Some(resources) = resources {
            page.set("Resources", resources);
        }
        for (key, value) in extra.into_iter() {
            page.set(key, value);
        }
        let page = doc.add_object(page);
        doc.objects.insert(
            pages,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1, "Resources" => inherited,
            }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        doc
    }

    #[test]
    fn annotations_draw_the_images_of_their_appearances_onto_their_rectangles() {
        let mut doc = page_of(b"", Some(dictionary! {}), dictionary! {}, dictionary! {});
        let image = doc.add_object(gray_row(1, vec![0]));
        let form = |doc: &mut Pdf, content: &[u8]| {
            doc.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject", "Subtype" => "Form",
                    "BBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
                    "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
                },
                content.to_vec(),
            ))
        };
        let drawn = form(&mut doc, b"q 10 0 0 10 0 0 cm /Im0 Do Q");
        let empty = form(&mut doc, b"");
        let rect = || vec![100.into(), 200.into(), 300.into(), 400.into()];
        let stamp = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Stamp", "Rect" => rect(), "AP" => dictionary! { "N" => drawn },
        });
        let hidden = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Stamp", "Rect" => rect(), "F" => 2, "AP" => dictionary! { "N" => drawn },
        });
        // A check box shows the appearance of its state, here the empty one.
        let check = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Rect" => rect(), "AS" => "Off",
            "AP" => dictionary! { "N" => dictionary! { "On" => drawn, "Off" => empty } },
        });
        let page = doc.get_pages()[&1];
        doc.get_dictionary_mut(page)
            .unwrap()
            .set("Annots", vec![stamp.into(), hidden.into(), check.into()]);
        let placed = placements(&doc, 1 << 20).unwrap();
        assert_eq!(placed.images.len(), 1);
        let matrices: Vec<_> = placed.pages[0]
            .1
            .iter()
            .map(|p| p.matrix.unwrap())
            .collect();
        // The form's box, 10 square, onto the 200 square at (100, 200).
        assert_eq!(matrices, [[200.0, 0.0, 0.0, 200.0, 100.0, 200.0]]);
    }

    #[test]
    fn images_of_inherited_resources_sizes_in_objects_and_unreadable_content_are_found() {
        let mut doc = page_of(b"", None, dictionary! {}, dictionary! {});
        let width = doc.add_object(Object::Integer(2));
        let mut image = gray_row(2, vec![0, 255]);
        image.dict.set("Width", width);
        let image = doc.add_object(image);
        // The page takes its resources from the page tree, written there,
        // and its content holds a stray delimiter before the image.
        let pages = doc
            .catalog()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_dictionary_mut(pages).unwrap().set(
            "Resources",
            dictionary! { "XObject" => dictionary! { "Im0" => image } },
        );
        let page = doc.get_pages()[&1];
        let contents = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.objects.insert(
            contents,
            Object::Stream(Stream::new(
                dictionary! {},
                b"1 ] ) q 4 0 0 2 0 0 cm /Im0 Do Q".to_vec(),
            )),
        );
        let placed = placements(&doc, 1 << 20).unwrap();
        assert_eq!(
            placed.pages[0]
                .1
                .iter()
                .map(|p| p.matrix.unwrap())
                .collect::<Vec<_>>(),
            [[4.0, 0.0, 0.0, 2.0, 0.0, 0.0]]
        );
        let images = read(&doc, 1 << 20).unwrap();
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (0, vec![0, 0, 255])
        );
    }

    #[test]
    fn an_inline_image_ending_its_stream_or_holding_ei_bytes_is_read() {
        // The data of an unfiltered image holds " EI ", which its length
        // steps over; a filtered one is the last thing its form draws.
        let mut samples = vec![255u8; 32 * 32];
        samples[..4].copy_from_slice(b" EI ");
        let mut content = b"BI /W 32 /H 32 /CS /G /BPC 8 ID ".to_vec();
        content.extend(&samples);
        content.extend(b"\nEI /Fm0 Do");
        let mut doc = page_of(
            &content,
            Some(dictionary! {}),
            dictionary! {},
            dictionary! {},
        );
        let mut form = b"BI /W 32 /H 32 /CS /G /BPC 8 /F /AHx ID\n".to_vec();
        form.extend("00".repeat(32 * 32).as_bytes());
        form.extend(b">\nEI");
        let form = doc.add_object(Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 1.into(), 1.into()] },
            form,
        ));
        let page = doc.get_pages()[&1];
        doc.get_dictionary_mut(page).unwrap().set(
            "Resources",
            dictionary! { "XObject" => dictionary! { "Fm0" => form } },
        );
        let images = read(&doc, 1 << 20).unwrap();
        assert_eq!(
            images.files.len(),
            2,
            "{:?} {}",
            images.files.keys(),
            images.note
        );
        let (_, first) = png_pixels(&images.files["image-001.png"]);
        assert_eq!(&first[1..6], b" EI \xff");
        let (_, second) = png_pixels(&images.files["image-002.png"]);
        assert!(second.iter().skip(1).take(32).all(|p| *p == 0));
    }

    fn form_box() -> Vec<Object> {
        vec![0.into(), 0.into(), 10.into(), 10.into()]
    }

    #[test]
    fn images_of_tiling_patterns_and_soft_masks_are_found_but_not_drawn_on_the_view() {
        let mut doc = page_of(b"", None, dictionary! {}, dictionary! {});
        let tile = doc.add_object(gray_row(2, vec![0, 255]));
        let shape = doc.add_object(gray_row(2, vec![255, 0]));
        let pattern = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "Pattern", "PatternType" => 1, "PaintType" => 1, "TilingType" => 1,
                "BBox" => form_box(), "XStep" => 10, "YStep" => 10,
                "Resources" => dictionary! { "XObject" => dictionary! { "T" => tile } },
            },
            b"q 10 0 0 10 0 0 cm /T Do Q".to_vec(),
        ));
        let group = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form", "BBox" => form_box(),
                "Group" => dictionary! { "S" => "Transparency" },
                "Resources" => dictionary! { "XObject" => dictionary! { "S" => shape } },
            },
            b"q 10 0 0 10 0 0 cm /S Do Q".to_vec(),
        ));
        let page = doc.get_pages()[&1];
        let contents = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.objects.insert(
            contents,
            Object::Stream(Stream::new(
                dictionary! {},
                b"/Pattern cs /P0 scn 0 0 10 10 re f /P0 scn 0 0 5 5 re f /GS0 gs 0 0 10 10 re f"
                    .to_vec(),
            )),
        );
        doc.get_dictionary_mut(page).unwrap().set(
            "Resources",
            dictionary! {
                "Pattern" => dictionary! { "P0" => pattern },
                "ExtGState" => dictionary! { "GS0" => dictionary! {
                    "SMask" => dictionary! { "S" => "Luminosity", "G" => group },
                } },
            },
        );
        let placed = placements(&doc, 1 << 20).unwrap();
        assert_eq!(placed.images.len(), 2);
        // The pattern used twice gives its image once.
        let found: Vec<_> = placed.pages[0]
            .1
            .iter()
            .map(|p| (p.index, p.matrix))
            .collect();
        assert_eq!(found, [(0, None), (1, None)]);
        let images = read(&doc, 1 << 20).unwrap();
        assert_eq!(images.pages["image-001.png"], [1]);
        assert_eq!(images.pages["image-002.png"], [1]);
    }

    #[test]
    fn a_picture_in_strips_is_one_image_and_a_flush_picture_of_another_size_stays_apart() {
        let mut doc = page_of(b"", None, dictionary! {}, dictionary! {});
        let top = doc.add_object(gray_row(2, vec![0, 255]));
        let bottom = doc.add_object(gray_row(2, vec![255, 0]));
        let other = doc.add_object(gray_row(4, vec![0, 0, 0, 0]));
        let page = doc.get_pages()[&1];
        let contents = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.objects.insert(
            contents,
            Object::Stream(Stream::new(
                dictionary! {},
                b"q 20 0 0 10 0 10 cm /A Do Q q 20 0 0 10 0 0 cm /B Do Q q 20 0 0 10 0 20 cm /C Do Q"
                    .to_vec(),
            )),
        );
        doc.get_dictionary_mut(page).unwrap().set(
            "Resources",
            dictionary! { "XObject" => dictionary! { "A" => top, "B" => bottom, "C" => other } },
        );
        let placed = placements(&doc, 1 << 20).unwrap();
        assert_eq!(placed.images.len(), 2);
        let found: Vec<_> = placed.pages[0]
            .1
            .iter()
            .map(|p| (p.index, p.matrix.unwrap()))
            .collect();
        assert_eq!(
            found,
            [
                (0, [20.0, 0.0, 0.0, 20.0, 0.0, 0.0]),
                (1, [20.0, 0.0, 0.0, 10.0, 0.0, 20.0])
            ]
        );
        // The strips as one picture, the top one above, each row filtered 0.
        let images = read(&doc, 1 << 20).unwrap();
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (0, vec![0, 0, 255, 0, 255, 0])
        );
    }

    #[test]
    fn the_images_of_type3_glyphs_are_placed_by_the_text_state_and_made_one_picture() {
        let mut doc = page_of(b"", None, dictionary! {}, dictionary! {});
        let mark = doc.add_object(Stream::new(
            dictionary! {
                "Subtype" => "Image", "Width" => 1, "Height" => 1, "ImageMask" => true,
            },
            vec![0],
        ));
        let glyph = doc.add_object(Stream::new(
            dictionary! {},
            b"10 0 0 0 10 10 d1 q 10 0 0 10 0 0 cm /M Do Q".to_vec(),
        ));
        let font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type3", "FontBBox" => form_box(),
            "FontMatrix" => vec![0.1.into(), 0.into(), 0.into(), 0.1.into(), 0.into(), 0.into()],
            "CharProcs" => dictionary! { "g0" => glyph },
            "Encoding" => dictionary! { "Differences" => vec![97.into(), "g0".into()] },
            "FirstChar" => 97, "LastChar" => 97, "Widths" => vec![10.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "M" => mark } },
        });
        let page = doc.get_pages()[&1];
        let contents = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.objects.insert(
            contents,
            Object::Stream(Stream::new(
                dictionary! {},
                b"BT /T3 20 Tf 100 200 Td (aa) Tj ET".to_vec(),
            )),
        );
        doc.get_dictionary_mut(page).unwrap().set(
            "Resources",
            dictionary! { "Font" => dictionary! { "T3" => font } },
        );
        // Each glyph a 20 point square, the second where the first's width
        // (10 glyph units, 2 points each) moves the text.
        let placed = placements(&doc, 1 << 20).unwrap();
        assert_eq!(placed.images.len(), 1);
        let found: Vec<_> = placed.pages[0]
            .1
            .iter()
            .map(|p| p.matrix.unwrap().map(|n| (n * 1000.0).round() / 1000.0))
            .collect();
        assert_eq!(found, [[40.0, 0.0, 0.0, 20.0, 100.0, 200.0]]);
        let images = read(&doc, 1 << 20).unwrap();
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (0, vec![0, 0, 0])
        );
    }

    /// A gray JPEG, 16 by 8: its left half black, its right half white.
    const HALF_BLACK_JPEG: &str = "ffd8ffe000104a46494600010100000100010000ffdb0043000101010101010101010101\
010101010101010101010101010101010101010101010101010101010101010101010101\
0101010101010101010101010101010101ffc0000b080008001001011100ffc400140001\
0000000000000000000000000000000bffc4001410010000000000000000000000000000\
0000ffda0008010100003f003ff3fc3fffd9";

    #[test]
    fn a_color_space_named_in_resources_and_a_jpeg_its_decode_inverts_are_read() {
        let mut doc = page_of(b"", None, dictionary! {}, dictionary! {});
        let mut named = gray_row(2, vec![0, 255]);
        named.dict.set("ColorSpace", "CS0");
        let named = doc.add_object(named);
        let jpeg: Vec<u8> = (0..HALF_BLACK_JPEG.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&HALF_BLACK_JPEG[i..i + 2], 16).unwrap())
            .collect();
        let inverted = doc.add_object(Stream::new(
            dictionary! {
                "Subtype" => "Image", "Width" => 16, "Height" => 8, "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8, "Filter" => "DCTDecode", "Decode" => vec![1.into(), 0.into()],
            },
            jpeg,
        ));
        let page = doc.get_pages()[&1];
        let contents = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.objects.insert(
            contents,
            Object::Stream(Stream::new(
                dictionary! {},
                b"q 2 0 0 1 0 0 cm /A Do Q q 16 0 0 8 0 100 cm /B Do Q".to_vec(),
            )),
        );
        doc.get_dictionary_mut(page).unwrap().set(
            "Resources",
            dictionary! {
                "ColorSpace" => dictionary! { "CS0" => "DeviceGray" },
                "XObject" => dictionary! { "A" => named, "B" => inverted },
            },
        );
        let images = read(&doc, 1 << 20).unwrap();
        assert_eq!(
            png_pixels(&images.files["image-001.png"]),
            (0, vec![0, 0, 255])
        );
        // The JPEG is decoded, its samples inverted: white on the left.
        let (color_type, pixels) = png_pixels(&images.files["image-002.png"]);
        assert_eq!((color_type, pixels.len()), (0, 8 * 17));
        let row = &pixels[1..17];
        assert!(row[..8].iter().all(|p| *p > 240), "{row:?}");
        assert!(row[8..].iter().all(|p| *p < 15), "{row:?}");
    }
}
