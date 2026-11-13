//! The images a PDF's pages draw, made into files that an image viewer and
//! Windows OCR open. They are imported as assets like the pictures of Word
//! and PowerPoint, so that a page scanned or saved from an image is read too.
//!
//! JPEG is kept as it is, CCITT fax data is wrapped in a TIFF, JPEG 2000 is
//! kept as a JP2 file, and samples are written as an 8-bit gray or RGB PNG.

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

/// The images the pages of `doc` draw, directly or in the Form XObjects they
/// draw, in the order the pages first draw them. An image drawn more than
/// once is one file. Inline images are left out.
pub(super) fn read(doc: &Pdf, max_page: usize) -> Result<ImageAssets> {
    let mut order = vec![];
    let mut drawn_on: BTreeMap<ObjectId, Vec<u32>> = BTreeMap::new();
    for (page, id) in doc.get_pages() {
        let raw = doc.get_page_content_with_limit(id, max_page)?;
        let content = match Content::decode_strict(&raw) {
            Ok(content) => content,
            Err(_) => Content::decode(&raw)?,
        };
        let resources = page_resources(doc, id);
        let mut found = vec![];
        drawn(
            doc,
            &content,
            &resources,
            0,
            &mut BTreeSet::new(),
            &mut found,
        );
        for image in found {
            let pages = drawn_on.entry(image).or_insert_with(|| {
                order.push(image);
                vec![]
            });
            if pages.last() != Some(&page) {
                pages.push(page);
            }
        }
    }
    let mut images = ImageAssets::default();
    let mut left_out: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for id in order {
        let pages = drawn_on.remove(&id).unwrap_or_default();
        match doc
            .get_object(id)
            .and_then(Object::as_stream)
            .map_err(anyhow::Error::from)
            .and_then(|stream| image_file(doc, stream))
        {
            Ok((bytes, extension)) => {
                let name = format!("image-{:03}.{extension}", images.files.len() + 1);
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

/// The images `content` draws with `Do`, and those the Form XObjects it
/// draws draw, each time it draws one.
fn drawn(
    doc: &Pdf,
    content: &Content,
    resources: &[&Dictionary],
    depth: usize,
    forms: &mut BTreeSet<ObjectId>,
    found: &mut Vec<ObjectId>,
) {
    for operation in &content.operations {
        if operation.operator != "Do" {
            continue;
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
            Ok(b"Image") => found.push(id),
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
                    drawn(doc, &inner, &resources, depth + 1, forms, found);
                }
                forms.remove(&id);
            }
            _ => {}
        }
    }
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
        Some(b"JPXDecode") => Ok((data, "jp2")),
        Some(b"CCITTFaxDecode" | b"CCF") => {
            Ok((ccitt_tiff(dict, params[general], &data, inverted)?, "tif"))
        }
        Some(_) => bail!("JBIG2で圧縮した画像"),
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

/// CCITT fax data as a one-strip TIFF, which Windows decodes. The pixels the
/// data codes as black are written black when the PDF draws them dark.
fn ccitt_tiff(
    dict: &Dictionary,
    params: Option<&Dictionary>,
    data: &[u8],
    inverted: bool,
) -> Result<Vec<u8>> {
    let param = |key: &[u8]| params.and_then(|p| integer(p, key));
    let flag = |key: &[u8]| params.and_then(|p| boolean(p, key)).unwrap_or(false);
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
    // A coded black pixel is the sample BlackIs1 gives it, which the Decode
    // array maps to dark when it does not invert that sample (an image mask
    // paints sample 0; a gray image draws sample 0 black).
    let dark = flag(b"BlackIs1") == inverted;
    let photometric: u32 = if dark { 0 } else { 1 };
    let (compression, options_tag, options): (u32, u16, u32) = if k < 0 {
        (4, 293, 0)
    } else {
        (
            3,
            292,
            u32::from(k > 0) | if flag(b"EncodedByteAlign") { 4 } else { 0 },
        )
    };
    let length = u32::try_from(data.len()).context("大きすぎるCCITTの画像")?;
    // Header, then the directory with its entries in tag order, then the data.
    let entries: [(u16, u16, u32); 10] = [
        (256, 4, columns),
        (257, 4, rows),
        (258, 3, 1),
        (259, 3, compression),
        (262, 3, photometric),
        (273, 4, 0), // the data's offset, set below
        (277, 3, 1),
        (278, 4, rows),
        (279, 4, length),
        (options_tag, 4, options),
    ];
    let offset = 8 + 2 + entries.len() as u32 * 12 + 4;
    let mut tiff = b"II*\0".to_vec();
    tiff.extend(8u32.to_le_bytes());
    tiff.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, value) in entries {
        let value = if tag == 273 { offset } else { value };
        tiff.extend(tag.to_le_bytes());
        tiff.extend(kind.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        if kind == 3 {
            tiff.extend((value as u16).to_le_bytes());
            tiff.extend([0, 0]);
        } else {
            tiff.extend(value.to_le_bytes());
        }
    }
    tiff.extend(0u32.to_le_bytes());
    tiff.extend_from_slice(data);
    Ok(tiff)
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

    #[test]
    fn jpeg_is_kept_ccitt_is_wrapped_and_jbig2_is_named_in_the_note() {
        let jpeg = Stream::new(
            dictionary! { "Subtype" => "Image", "Width" => 1, "Height" => 1, "Filter" => "DCTDecode" },
            b"\xff\xd8jpeg".to_vec(),
        );
        let images = read(&drawing(jpeg), 1 << 20).unwrap();
        assert_eq!(images.files["image-001.jpg"], b"\xff\xd8jpeg");
        let fax = Stream::new(
            dictionary! {
                "Subtype" => "Image", "Width" => 8, "Height" => 2, "Filter" => "CCITTFaxDecode",
                "DecodeParms" => dictionary! { "K" => -1, "Columns" => 8 },
            },
            b"g4".to_vec(),
        );
        let images = read(&drawing(fax), 1 << 20).unwrap();
        let tiff = &images.files["image-001.tif"];
        assert!(tiff.starts_with(b"II*\0") && tiff.ends_with(b"g4"));
        let jbig2 = Stream::new(
            dictionary! { "Subtype" => "Image", "Width" => 1, "Height" => 1, "Filter" => "JBIG2Decode" },
            b"jb2".to_vec(),
        );
        let images = read(&drawing(jbig2), 1 << 20).unwrap();
        assert!(images.files.is_empty());
        assert!(
            images.note.contains("1ページ（JBIG2で圧縮した画像）"),
            "{}",
            images.note
        );
    }
}
