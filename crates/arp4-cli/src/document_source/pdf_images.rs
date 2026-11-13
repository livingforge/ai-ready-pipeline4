//! The images a PDF's pages draw, made into files that an image viewer and
//! Windows OCR open. They are imported as assets like the pictures of Word
//! and PowerPoint, so that a page scanned or saved from an image is read too.
//!
//! JPEG is kept as it is; samples, CCITT fax, JBIG2 and JPEG 2000 images are
//! decoded and written as 8-bit gray or RGB PNG, which a browser shows too.

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

/// An image a page draws: an image XObject, or an inline image with its
/// dictionary written out in full.
pub(super) enum Drawn {
    Object(ObjectId),
    Inline(Box<Stream>),
}

/// An image a page draws: its index among the images of the document, in
/// the order the pages first draw them, the matrix that maps its unit square
/// onto the page (in the page's user space), and whether it is a stencil
/// mask, which paints only where its samples say.
pub struct Placement {
    pub index: usize,
    pub matrix: [f64; 6],
    pub mask: bool,
}

/// The images of a PDF by their index, and the images each page (by its
/// number) draws.
pub(super) struct Placements {
    pub images: Vec<Drawn>,
    pub pages: Vec<(u32, Vec<Placement>)>,
}

/// The images the pages of `doc` draw, directly or in the Form XObjects they
/// draw, page by page, and each image by its index. An image drawn more than
/// once, and inline images alike in data and dictionary, have one index.
/// Inline images too small to hold text are left out.
pub(super) fn placements(doc: &Pdf, max_page: usize) -> Result<Placements> {
    let mut order = vec![];
    let mut objects: BTreeMap<ObjectId, usize> = BTreeMap::new();
    let mut inline: BTreeMap<(String, Vec<u8>), usize> = BTreeMap::new();
    let mut pages = vec![];
    for (page, id) in doc.get_pages() {
        let raw = doc.get_page_content_with_limit(id, max_page)?;
        let content = match Content::decode_strict(&raw) {
            Ok(content) => content,
            Err(_) => Content::decode(&raw)?,
        };
        let mut found = vec![];
        drawn(
            doc,
            &content,
            &raw,
            &page_resources(doc, id),
            IDENTITY,
            0,
            &mut BTreeSet::new(),
            &mut found,
        );
        let placed = found
            .into_iter()
            .map(|(image, matrix, mask)| {
                let next = order.len();
                let index = match &image {
                    Drawn::Object(id) => *objects.entry(*id).or_insert(next),
                    Drawn::Inline(stream) => *inline
                        .entry((format!("{:?}", stream.dict), stream.content.clone()))
                        .or_insert(next),
                };
                if index == next {
                    order.push(image);
                }
                Placement {
                    index,
                    matrix,
                    mask,
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
        let stream = match &image {
            Drawn::Object(id) => doc.get_object(*id).and_then(Object::as_stream),
            Drawn::Inline(stream) => Ok(&**stream),
        };
        match stream
            .map_err(anyhow::Error::from)
            .and_then(|stream| image_file(doc, stream))
        {
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

/// The resource dictionaries of page `id`: its own, then those it inherits.
fn page_resources(doc: &Pdf, id: ObjectId) -> Vec<&Dictionary> {
    let Ok((own, inherited)) = doc.get_page_resources(id) else {
        return vec![];
    };
    own.into_iter()
        .chain(
            inherited
                .into_iter()
                .filter_map(|id| doc.get_dictionary(id).ok()),
        )
        .collect()
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

/// The images `content` (decoded from `raw`) draws when `ctm` maps its space
/// onto the page, with `Do` and inline, and those the Form XObjects it draws
/// draw, each time it draws one, with the matrix mapping the image's unit
/// square onto the page and whether it is a stencil mask.
#[allow(clippy::too_many_arguments)]
fn drawn(
    doc: &Pdf,
    content: &Content,
    raw: &[u8],
    resources: &[&Dictionary],
    ctm: [f64; 6],
    depth: usize,
    forms: &mut BTreeSet<ObjectId>,
    found: &mut Vec<(Drawn, [f64; 6], bool)>,
) {
    let mut ctm = ctm;
    let mut saved = vec![];
    // The byte ranges of the inline images, found when the first is drawn.
    let mut inline_ranges = None;
    let mut inline_count = 0;
    for operation in &content.operations {
        match operation.operator.as_str() {
            "q" => saved.push(ctm),
            "Q" => ctm = saved.pop().unwrap_or(ctm),
            "cm" => {
                if let Some(m) = matrix(&operation.operands) {
                    ctm = super::pdf_multiply(m, ctm);
                }
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
                    inline_image(doc, &raw[range], operation.operands.first(), resources)
                }) {
                    let mask = boolean(&stream.dict, b"ImageMask").unwrap_or(false);
                    found.push((Drawn::Inline(Box::new(stream)), ctm, mask));
                }
                continue;
            }
            "Do" => {}
            _ => continue,
        }
        let Some(Ok(name)) = operation.operands.first().map(Object::as_name) else {
            continue;
        };
        let Some(id) = resources.iter().find_map(|resources| {
            dictionary(doc, resources.get(b"XObject").ok()?)?
                .get(name)
                .ok()?
                .as_reference()
                .ok()
        }) else {
            continue;
        };
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
            continue;
        };
        match stream.dict.get(b"Subtype").and_then(Object::as_name) {
            Ok(b"Image") => {
                let mask = boolean(&stream.dict, b"ImageMask").unwrap_or(false);
                found.push((Drawn::Object(id), ctm, mask));
            }
            Ok(b"Form") if depth < MAX_FORM_DEPTH && forms.insert(id) => {
                if let Ok(raw) = stream.decompressed_content_with_limit(MAX_SAMPLES)
                    && let Ok(inner) = Content::decode(&raw)
                {
                    // A form without resources of its own uses those of the
                    // page drawing it, as older PDFs write.
                    let own = stream
                        .dict
                        .get(b"Resources")
                        .ok()
                        .and_then(|r| dictionary(doc, r));
                    let resources = match own {
                        Some(own) => vec![own],
                        None => resources.to_vec(),
                    };
                    let form_matrix = stream
                        .dict
                        .get(b"Matrix")
                        .ok()
                        .and_then(|m| m.as_array().ok())
                        .and_then(|m| matrix(m))
                        .unwrap_or(IDENTITY);
                    drawn(
                        doc,
                        &inner,
                        &raw,
                        &resources,
                        super::pdf_multiply(form_matrix, ctm),
                        depth + 1,
                        forms,
                        found,
                    );
                }
                forms.remove(&id);
            }
            _ => {}
        }
    }
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
    let width = integer(&dict, b"Width")?;
    let height = integer(&dict, b"Height")?;
    if width.min(height) < MIN_INLINE_SIDE || width * height < MIN_INLINE_PIXELS {
        return None;
    }
    Some(Stream::new(dict, data))
}

fn integer(dict: &Dictionary, key: &[u8]) -> Option<i64> {
    dict.get(key).and_then(Object::as_i64).ok()
}

fn boolean(dict: &Dictionary, key: &[u8]) -> Option<bool> {
    dict.get(key).and_then(Object::as_bool).ok()
}

/// The file of an image XObject and its extension.
fn image_file(doc: &Pdf, stream: &Stream) -> Result<(Vec<u8>, &'static str)> {
    let dict = &stream.dict;
    let filters: Vec<&[u8]> = match dict.get(b"Filter") {
        Ok(_) => stream.filters().context("フィルターを読めない画像")?,
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
    let data = unfilter(&stream.content, &filters[..general], &params[..general])
        .context("圧縮を解けない画像")?;
    let inverted = inverted(doc, dict);
    match codec {
        Some(b"DCTDecode" | b"DCT") => Ok((data, "jpg")),
        Some(b"JPXDecode") => Ok((jpx_png(&data)?, "png")),
        Some(b"CCITTFaxDecode" | b"CCF") => {
            Ok((ccitt_png(dict, params[general], &data, inverted)?, "png"))
        }
        Some(_) => Ok((jbig2_png(doc, params[general], &data, inverted)?, "png")),
        None => Ok((samples_png(doc, dict, data, inverted)?, "png")),
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

    /// The pixels as a PNG `height` rows high, rows not decoded left white.
    fn png(mut self, height: usize) -> Result<Vec<u8>> {
        ensure!(!self.pixels.is_empty(), "画素を読めない画像");
        let white = self.color(false);
        self.pixels.resize(self.width * height, white);
        Ok(png(self.width as u32, height as u32, 1, &self.pixels))
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

/// CCITT fax data as a gray PNG. The pixels the data codes as black are
/// black when the PDF draws them dark.
fn ccitt_png(
    dict: &Dictionary,
    params: Option<&Dictionary>,
    data: &[u8],
    inverted: bool,
) -> Result<Vec<u8>> {
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
    image.png(rows as usize)
}

/// JBIG2 data, with the global segments its parameters name, as a gray PNG.
/// JBIG2 codes black as 1, which the filter gives as sample 0.
fn jbig2_png(
    doc: &Pdf,
    params: Option<&Dictionary>,
    data: &[u8],
    inverted: bool,
) -> Result<Vec<u8>> {
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
    image.png(height as usize)
}

/// JPEG 2000 data as an 8-bit gray or RGB PNG, without its alpha channel.
fn jpx_png(data: &[u8]) -> Result<Vec<u8>> {
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
    for pixel in samples.chunks_exact(stride).take(pixels) {
        space.write(&pixel[..components], &mut out);
    }
    Ok(png(width, height, space.channels(), &out))
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
}

impl Space {
    fn components(&self) -> usize {
        match self {
            Self::Gray | Self::Ink | Self::Indexed(..) => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
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
                    b"DeviceN"
                        if entries
                            .get(1)
                            .and_then(|names| resolved(doc, names).as_array().ok())
                            .is_some_and(|names| names.len() == 1) =>
                    {
                        Self::Ink
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
            Self::Rgb | Self::Cmyk => 3,
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
        }
    }
}

/// An image's samples as an 8-bit gray or RGB PNG.
fn samples_png(doc: &Pdf, dict: &Dictionary, mut data: Vec<u8>, inverted: bool) -> Result<Vec<u8>> {
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
    let mut pixels = Vec::with_capacity(width * height * channels);
    let mut color = vec![0u8; components];
    for line in data.chunks_exact(row) {
        let mut bit = 0;
        for _ in 0..width {
            for component in color.iter_mut() {
                let sample = match bits {
                    8 => u32::from(line[bit / 8]),
                    16 => u32::from(line[bit / 8]),
                    _ => (u32::from(line[bit / 8]) >> (8 - bits - bit % 8)) & max,
                };
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
        }
    }
    Ok(png(width as u32, height as u32, channels, &pixels))
}

/// 8-bit gray (one channel) or RGB (three) pixels as a PNG.
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
    header.extend([8, if channels == 1 { 0 } else { 2 }, 0, 0, 0]);
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
            .map(|p| (p.index, p.matrix, p.mask))
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
        let placed: Vec<_> = pages[0].1.iter().map(|p| (p.index, p.matrix[0])).collect();
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
        let png = samples_png(&doc, &mask, vec![0b1010_0000], inverted(&doc, &mask)).unwrap();
        assert_eq!(png_pixels(&png), (0, vec![0, 0, 255, 0]));
        let indexed = dictionary! {
            "Width" => 2, "Height" => 1, "BitsPerComponent" => 8,
            "ColorSpace" => vec!["Indexed".into(), "DeviceGray".into(), 1.into(), Object::string_literal(vec![10u8, 20])],
        };
        let png = samples_png(&doc, &indexed, vec![1, 0], false).unwrap();
        assert_eq!(png_pixels(&png), (0, vec![0, 20, 10]));
        let cmyk = dictionary! {
            "Width" => 1, "Height" => 1, "BitsPerComponent" => 8, "ColorSpace" => "DeviceCMYK",
        };
        let png = samples_png(&doc, &cmyk, vec![0, 255, 255, 0], false).unwrap();
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
        assert_eq!(images.files["image-001.jpg"], b"\xff\xd8jpeg");
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
        let lab = Stream::new(
            dictionary! {
                "Subtype" => "Image", "Width" => 1, "Height" => 1, "BitsPerComponent" => 8,
                "ColorSpace" => vec!["Lab".into(), dictionary! {}.into()],
            },
            vec![0, 0, 0],
        );
        let images = read(&drawing(lab), 1 << 20).unwrap();
        assert!(images.files.is_empty());
        assert!(
            images.note.contains("1ページ（色空間Labの画像）"),
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
}
