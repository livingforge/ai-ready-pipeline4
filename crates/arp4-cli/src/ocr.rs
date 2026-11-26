//! Automatic OCR for imported image assets on Windows. The image is read as
//! 8-bit BGRA, made small enough for the engine, and laid over a backdrop
//! where it is transparent, since the engine reads the colors alone and a
//! transparent picture's hidden colors are often all black. A metafile (EMF,
//! WMF: a picture pasted from Excel, PowerPoint or Visio, the view of an
//! embedded object) is drawn on white by Windows first; an SVG picture's text
//! is read from its text elements, as it is text rather than pixels.
//!
//! The engine gives each word where it stands in the picture. The words are
//! laid out as the strings of a PDF page are
//! ([`crate::document_source::page_layout`]): those of a line are one text,
//! the lines a picture rules are a table whose cells hold the words inside
//! them, and words set in the same columns line after line are a table
//! without rulings. The text of the result is read in that order, a row a
//! line and its cells a tab apart; the layout records each cell with where
//! it stands.
use crate::document_source::page_layout::{self, Rule, Shown};
use anyhow::Result;
use serde_json::{Value, json};

pub fn recognize(name: &str, bytes: &[u8]) -> Value {
    if svg(name, bytes) {
        return match svg_text(bytes) {
            Ok(text) => json!({
                "status":"available",
                "text":text,
                "reason":"text of the SVG picture's text elements, read as written"
            }),
            Err(error) => json!({
                "status":"unavailable",
                "text":"",
                "reason":format!("the SVG picture could not be read: {error:#}")
            }),
        };
    }
    match run(bytes) {
        Ok(read) => {
            let (text, layout) = laid(&read);
            let mut result = json!({
                "status":"available",
                "text":text,
                "reason":format!("Windows OCR ({}) executed on imported image", read.language)
            });
            if let Some(layout) = layout {
                result["layout"] = layout;
            }
            result
        }
        Err(error) => json!({
            "status":"unavailable",
            "text":"",
            "reason":format!("Windows OCR could not process imported image: {error:#}")
        }),
    }
}

/// What the engine made out of a picture: the language it read in, the size
/// of the picture as it read it, the words of each line with where each
/// stands (`[x, y, width, height]` in pixels from the top left), how far
/// the text is turned clockwise, in degrees, and the opaque BGRA pixels it
/// read.
#[cfg_attr(not(windows), allow(dead_code))]
struct Read {
    language: String,
    width: u32,
    height: u32,
    lines: Vec<Vec<(String, [f64; 4])>>,
    angle: f64,
    pixels: Vec<u8>,
}

/// The size of text the layout's measures are made for: a picture's words
/// are laid out at the scale its lines of text are this high.
const TEXT_SIZE: f64 = 10.0;
/// The most pixels a picture's rulings are looked for in.
const MAX_RULED_PIXELS: usize = 80_000_000;
/// The furthest, in degrees, a picture may be turned and its rulings still
/// run along its rows and columns of pixels.
const LEVEL: f64 = 0.3;

/// The text of what was `read`, in the order it is read, and its layout:
/// none when no word was made out.
fn laid(read: &Read) -> (String, Option<Value>) {
    // Each word with the bottom and height of its line. A line the engine
    // read downwards, as it reads the words of a narrow column, is no line
    // of the page: each of its words stands by itself.
    let mut words: Vec<(&str, [f64; 4], f64, f64)> = vec![];
    let mut heights = vec![];
    for line in &read.lines {
        let fold = |pick: fn(&[f64; 4]) -> f64, least: bool| {
            line.iter().map(|(_, r)| pick(r)).fold(
                if least { f64::MAX } else { f64::MIN },
                if least { f64::min } else { f64::max },
            )
        };
        let (left, right) = (fold(|r| r[0], true), fold(|r| r[0] + r[2], false));
        let (top, bottom) = (fold(|r| r[1], true), fold(|r| r[1] + r[3], false));
        if line.is_empty() || bottom <= top {
            continue;
        }
        if line.len() > 1 && bottom - top > 1.5 * (right - left) {
            for (text, rect) in line {
                heights.push(rect[3]);
                words.push((text, *rect, rect[1] + rect[3], rect[3]));
            }
            continue;
        }
        heights.push(bottom - top);
        for (text, rect) in line {
            words.push((text, *rect, bottom, bottom - top));
        }
    }
    if words.is_empty() {
        return (String::new(), None);
    }
    heights.sort_by(f64::total_cmp);
    let em = heights[heights.len() / 2].max(1.0);
    let scale = TEXT_SIZE / em;
    // The text is turned back level about the middle of the picture.
    let (sin, cos) = (-read.angle).to_radians().sin_cos();
    let centre = [f64::from(read.width) / 2.0, f64::from(read.height) / 2.0];
    let shown: Vec<Shown<'_>> = words
        .iter()
        .map(|(text, rect, bottom, height)| {
            // A line of low marks alone, as a dash is, stands at mid-height.
            let (size, baseline) = if *height < 0.6 * em {
                (em, bottom - height / 2.0 + 0.4 * em)
            } else {
                (*height, *bottom)
            };
            let (dx, dy) = (rect[0] - centre[0], baseline - centre[1]);
            Shown {
                text,
                x: (centre[0] + dx * cos - dy * sin) * scale,
                y: (centre[1] + dx * sin + dy * cos) * scale,
                width: rect[2] * scale,
                size: size * scale,
                upright: true,
                invisible: false,
                unit: None,
            }
        })
        .collect();
    let rules = if read.angle.abs() <= LEVEL {
        pixel_rules(read.width as usize, read.height as usize, &read.pixels, em)
            .into_iter()
            .map(|rule| Rule {
                level: rule.level,
                at: rule.at * scale,
                from: rule.from * scale,
                to: rule.to * scale,
            })
            .collect()
    } else {
        vec![]
    };
    let layout = page_layout::ruled_layout(&shown, rules, &[], &[], &[]);
    // The lines of a cell are one text on its row.
    let flat = |text: &str| {
        let mut out = String::new();
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                let apart = out.chars().next_back().zip(line.chars().next());
                if apart
                    .is_some_and(|(a, b)| !page_layout::unspaced(a) && !page_layout::unspaced(b))
                {
                    out.push(' ');
                }
            }
            out.push_str(line);
        }
        out
    };
    let mut text = String::new();
    let mut cells = vec![];
    let (mut row, mut column) = (0, 1);
    for cell in &layout.cells {
        let value = page_layout::composed(&cell.pieces, |i| words[i].0);
        if cell.row != row {
            if row > 0 {
                text.push('\n');
            }
            (row, column) = (cell.row, 1);
        }
        for _ in column..cell.column {
            text.push('\t');
        }
        column = cell.column;
        text.push_str(&flat(&value));
        let bounds = cell.pieces.iter().map(|piece| words[piece.string].1).fold(
            [f64::MAX, f64::MAX, f64::MIN, f64::MIN],
            |b, r| {
                [
                    b[0].min(r[0]),
                    b[1].min(r[1]),
                    b[2].max(r[0] + r[2]),
                    b[3].max(r[1] + r[3]),
                ]
            },
        );
        let name = crate::excel::column_name(cell.column).expect("a column of a sheet");
        cells.push(json!({
            "address":format!("{name}{}", cell.row),
            "text":value,
            "box":[
                bounds[0].round() as i64,
                bounds[1].round() as i64,
                (bounds[2] - bounds[0]).round() as i64,
                (bounds[3] - bounds[1]).round() as i64
            ]
        }));
    }
    let layout = json!({
        "size":[read.width, read.height],
        "cells":cells,
        "merges":layout.merges,
        "tables":layout.tables
    });
    (text, Some(layout))
}

/// The lines a picture of `width` by `height` opaque BGRA `pixels` rules:
/// runs of pixels darker than its ground (lighter on a dark ground) along a
/// row or column, three lines of text of height `em` long or longer, with
/// something lighter than themselves a little to both sides along most of
/// their length. A line has that on both sides, be it the ground or the
/// shade of a heading it rules off; a filled area has more of itself on
/// one. In pixels.
fn pixel_rules(width: usize, height: usize, pixels: &[u8], em: f64) -> Vec<Rule> {
    if width == 0
        || height == 0
        || width * height > MAX_RULED_PIXELS
        || pixels.len() < width * height * 4
    {
        return vec![];
    }
    let mut luma: Vec<i16> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .take(width * height)
        .map(|p| {
            ((114 * u32::from(p[0]) + 587 * u32::from(p[1]) + 299 * u32::from(p[2])) / 1000) as i16
        })
        .collect();
    // The ground is the shade most of the picture has; a picture on a dark
    // ground is read as its negative.
    let mut shades = [0usize; 256];
    for value in &luma {
        shades[*value as usize] += 1;
    }
    let mut ground = (0..256).max_by_key(|shade| shades[*shade]).unwrap_or(255) as i16;
    if ground < 128 {
        ground = 255 - ground;
        for value in &mut luma {
            *value = 255 - *value;
        }
    }
    /// How much darker than what it stands on a line is.
    const APART: i16 = 40;
    let inked = ground - APART.max(ground / 4);
    let least = (3.0 * em).max(24.0) as usize;
    // How far to each side of a run what it stands on is looked at.
    let beside = (0.35 * em).max(3.0).ceil() as usize + 1;
    let mut rules = vec![];
    // Along `count` lines of `length` pixels, the pixel `(line, along)` at
    // `at(line, along)`.
    let mut scan =
        |level: bool, count: usize, length: usize, at: &dyn Fn(usize, usize) -> usize| {
            for line in 0..count {
                let mut along = 0;
                while along < length {
                    if luma[at(line, along)] >= inked {
                        along += 1;
                        continue;
                    }
                    // A run, over gaps of a pixel or two.
                    let start = along;
                    let mut end = along + 1;
                    let mut next = end;
                    while next < length && next - end <= 2 {
                        if luma[at(line, next)] < inked {
                            end = next + 1;
                        }
                        next += 1;
                    }
                    along = end;
                    if end - start < least {
                        continue;
                    }
                    // Past the edge of the picture is its ground.
                    let lighter = |other: Option<usize>| {
                        other.filter(|other| *other < count).is_none_or(|other| {
                            let same = (start..end)
                                .filter(|i| luma[at(other, *i)] < luma[at(line, *i)] + APART)
                                .count();
                            2 * same < end - start
                        })
                    };
                    if lighter(line.checked_sub(beside)) && lighter(Some(line + beside)) {
                        rules.push(Rule {
                            level,
                            at: line as f64 + 0.5,
                            from: start as f64,
                            to: end as f64,
                        });
                    }
                }
            }
        };
    scan(true, height, width, &|row, column| row * width + column);
    scan(false, width, height, &|column, row| row * width + column);
    rules
}

/// Whether an image (by its asset name and bytes) is an SVG picture.
fn svg(name: &str, bytes: &[u8]) -> bool {
    let named = std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
    named
        || (head
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with('<')
            && head.contains("<svg"))
}

/// The text an SVG picture draws: that of each of its `text` elements, in
/// the order it draws them, a line each; not the title, description or
/// metadata it holds for readers.
fn svg_text(bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(bytes)?.trim_start_matches('\u{feff}');
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let xml = roxmltree::Document::parse_with_options(text, options)?;
    let unshown = |node: roxmltree::Node<'_, '_>| {
        node.ancestors().any(|a| {
            matches!(
                a.tag_name().name(),
                "title" | "desc" | "metadata" | "style" | "script" | "defs"
            )
        })
    };
    let lines: Vec<String> = xml
        .descendants()
        .filter(|n| n.tag_name().name() == "text" && !unshown(*n))
        .map(|n| {
            n.descendants()
                .filter(|t| t.is_text())
                .filter_map(|t| t.text())
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect();
    Ok(lines.join("\n"))
}

#[cfg(not(windows))]
fn run(_bytes: &[u8]) -> Result<Read> {
    anyhow::bail!("Windows OCR requires Windows")
}

#[cfg(windows)]
fn run(bytes: &[u8]) -> Result<Read> {
    use std::{sync::mpsc, time::Duration};
    let bytes = bytes.to_vec();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(recognize_winrt(&bytes).map_err(anyhow::Error::from));
    });
    receiver
        .recv_timeout(Duration::from_secs(60))
        .map_err(|_| anyhow::anyhow!("OCR timed out after 60 seconds"))?
}

#[cfg(windows)]
fn recognize_winrt(bytes: &[u8]) -> windows::core::Result<Read> {
    use windows::{
        Graphics::Imaging::{
            BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat,
            BitmapTransform, ColorManagementMode, ExifOrientationMode, SoftwareBitmap,
        },
        Media::Ocr::OcrEngine,
        Storage::Streams::{Buffer, DataReader, DataWriter, InMemoryRandomAccessStream},
        core::{Error, HRESULT},
    };
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
        Error::new(
            HRESULT(0x80004005u32 as i32),
            "No Windows OCR language matches the user profile.",
        )
    })?;
    // A metafile is drawn by Windows, on white, and read as drawn.
    let (width, height, pixels) = match metafile_pixels(bytes)? {
        Some(drawn) => drawn,
        None => {
            let stream = InMemoryRandomAccessStream::new()?;
            let writer = DataWriter::CreateDataWriter(&stream)?;
            writer.WriteBytes(bytes)?;
            writer.StoreAsync()?.join()?;
            writer.FlushAsync()?.join()?;
            writer.DetachStream()?;
            stream.Seek(0)?;
            let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
            // An image longer than the engine takes is scaled down to fit.
            let (width, height) = (
                decoder.OrientedPixelWidth()?,
                decoder.OrientedPixelHeight()?,
            );
            let longest = width.max(height).max(1);
            let limit = OcrEngine::MaxImageDimension()?;
            let transform = BitmapTransform::new()?;
            if longest > limit {
                let scale = |side: u32| {
                    (u64::from(side) * u64::from(limit) / u64::from(longest)).max(1) as u32
                };
                transform.SetScaledWidth(scale(width))?;
                transform.SetScaledHeight(scale(height))?;
                transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
            }
            let read = decoder
                .GetSoftwareBitmapTransformedAsync(
                    BitmapPixelFormat::Bgra8,
                    BitmapAlphaMode::Straight,
                    &transform,
                    ExifOrientationMode::RespectExifOrientation,
                    ColorManagementMode::DoNotColorManage,
                )?
                .join()?;
            let (width, height) = (read.PixelWidth()? as u32, read.PixelHeight()? as u32);
            let size = (width as usize) * (height as usize) * 4;
            let buffer = Buffer::Create(size as u32)?;
            read.CopyToBuffer(&buffer)?;
            read.Close()?;
            let mut pixels = vec![0u8; size];
            DataReader::FromBuffer(&buffer)?.ReadBytes(&mut pixels)?;
            // An image without alpha (JPEG, a PNG without it) is read as it
            // is: its alpha says nothing.
            if decoder.BitmapAlphaMode()? == BitmapAlphaMode::Ignore {
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel[3] = 255;
                }
            } else {
                flatten(&mut pixels);
            }
            (width, height, pixels)
        }
    };
    let writer = DataWriter::new()?;
    writer.WriteBytes(&pixels)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &writer.DetachBuffer()?,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let mut lines = vec![];
    for line in result.Lines()? {
        let mut words = vec![];
        for word in line.Words()? {
            let rect = word.BoundingRect()?;
            words.push((
                word.Text()?.to_string(),
                [
                    f64::from(rect.X),
                    f64::from(rect.Y),
                    f64::from(rect.Width),
                    f64::from(rect.Height),
                ],
            ));
        }
        lines.push(words);
    }
    // No angle is given for text the engine found level.
    let angle = result
        .TextAngle()
        .and_then(|angle| angle.Value())
        .unwrap_or(0.0);
    let language = engine.RecognizerLanguage()?.LanguageTag()?.to_string();
    bitmap.Close()?;
    Ok(Read {
        language,
        width,
        height,
        lines,
        angle,
        pixels,
    })
}

/// The longest side a metafile is drawn at, the longest it is drawn at
/// least (a small one, as the view of an embedded object a cell large, is
/// drawn larger: it is drawn from lines and text, not pixels), and the pixels
/// per inch it is drawn at between them.
#[cfg(windows)]
const METAFILE_SIDE: f64 = 4000.0;
#[cfg(windows)]
const METAFILE_LEAST_SIDE: f64 = 1600.0;
#[cfg(windows)]
const METAFILE_DPI: f64 = 200.0;

/// An EMF or WMF picture drawn on white by Windows (GDI) as opaque top-down
/// BGRA pixels, with its width and height; none for another image. A
/// placeable WMF is drawn as large as its header says, a WMF without one as
/// Windows takes it.
#[cfg(windows)]
fn metafile_pixels(bytes: &[u8]) -> windows::core::Result<Option<(u32, u32, Vec<u8>)>> {
    use windows::Win32::{
        Foundation::RECT,
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
            DIB_RGB_COLORS, DeleteDC, DeleteEnhMetaFile, DeleteObject, ENHMETAHEADER, GdiFlush,
            GetEnhMetaFileHeader, HGDIOBJ, PatBlt, PlayEnhMetaFile, SelectObject,
            SetEnhMetaFileBits, WHITENESS,
        },
        System::DataExchange::{METAFILEPICT, SetWinMetaFileBits},
    };
    let word = |at: usize| {
        bytes
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let emf = bytes.len() >= 44 && bytes[..4] == [1, 0, 0, 0] && &bytes[40..44] == b" EMF";
    let placeable = bytes.len() >= 40 && bytes[..4] == [0xd7, 0xcd, 0xc6, 0x9a];
    let standard = bytes.len() >= 18 && matches!(word(0), Some(1 | 2)) && word(2) == Some(9);
    // MM_ANISOTROPIC: the picture is stretched to the size it is played at.
    const ANISOTROPIC: i32 = 8;
    let handle = unsafe {
        if emf {
            SetEnhMetaFileBits(bytes)
        } else if placeable {
            // The bounding box in units of which `inch` make an inch, as
            // HIMETRIC (hundredths of a millimeter).
            let at = |offset: usize| i32::from(word(offset).unwrap_or(0) as i16);
            let inch = i32::from(word(14).unwrap_or(1440).max(1));
            let picture = METAFILEPICT {
                mm: ANISOTROPIC,
                xExt: (at(10) - at(6)).abs() * 2540 / inch,
                yExt: (at(12) - at(8)).abs() * 2540 / inch,
                hMF: Default::default(),
            };
            SetWinMetaFileBits(&bytes[22..], None, Some(&picture))
        } else if standard {
            let picture = METAFILEPICT {
                mm: ANISOTROPIC,
                ..Default::default()
            };
            SetWinMetaFileBits(bytes, None, Some(&picture))
        } else {
            return Ok(None);
        }
    };
    if handle.is_invalid() {
        return Err(windows::core::Error::from_thread());
    }
    let mut header = ENHMETAHEADER::default();
    unsafe {
        GetEnhMetaFileHeader(
            handle,
            std::mem::size_of::<ENHMETAHEADER>() as u32,
            Some(&mut header),
        )
    };
    // Its frame, in hundredths of a millimeter, at the drawing resolution;
    // else its bounds, in pixels.
    let frame = header.rclFrame;
    let bounds = header.rclBounds;
    let (mut width, mut height) = (
        f64::from(frame.right - frame.left) / 2540.0 * METAFILE_DPI,
        f64::from(frame.bottom - frame.top) / 2540.0 * METAFILE_DPI,
    );
    if width <= 0.0 || height <= 0.0 {
        (width, height) = (
            f64::from(bounds.right - bounds.left),
            f64::from(bounds.bottom - bounds.top),
        );
    }
    let longest = width.max(height).max(1.0);
    let scale = (METAFILE_LEAST_SIDE / longest)
        .max(1.0)
        .min(METAFILE_SIDE / longest);
    width *= scale;
    height *= scale;
    let (width, height) = (
        (width.round() as i32).clamp(1, METAFILE_SIDE as i32),
        (height.round() as i32).clamp(1, METAFILE_SIDE as i32),
    );
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: rows from the top down.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let drawn = unsafe {
        let dc = CreateCompatibleDC(None);
        let mut bits = std::ptr::null_mut();
        let drawn =
            CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).map(|bitmap| {
                let previous = SelectObject(dc, HGDIOBJ::from(bitmap));
                let _ = PatBlt(dc, 0, 0, width, height, WHITENESS);
                let _ = PlayEnhMetaFile(
                    dc,
                    handle,
                    &RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: height,
                    },
                );
                let _ = GdiFlush();
                let size = (width as usize) * (height as usize) * 4;
                let mut pixels = std::slice::from_raw_parts(bits as *const u8, size).to_vec();
                SelectObject(dc, previous);
                let _ = DeleteObject(HGDIOBJ::from(bitmap));
                // GDI leaves the alpha of what it draws 0: all is opaque.
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel[3] = 255;
                }
                pixels
            });
        let _ = DeleteDC(dc);
        let _ = DeleteEnhMetaFile(Some(handle));
        drawn?
    };
    Ok(Some((width as u32, height as u32, drawn)))
}

/// Lays 8-bit straight BGRA pixels over a backdrop where they are not
/// opaque, leaving them opaque: white under a dark picture, black under a
/// light one (white text on a transparent ground), as the picture shows
/// against what it is drawn over. Opaque pixels stay as they are.
#[cfg_attr(not(windows), allow(dead_code))]
fn flatten(pixels: &mut [u8]) {
    if pixels
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[3] == 255)
    {
        return;
    }
    let (mut covered, mut light) = (0u64, 0u64);
    for pixel in pixels.as_chunks::<4>().0 {
        let alpha = u64::from(pixel[3]);
        // Luma of the pixel's own color, weighted by how much of it shows.
        let luma =
            (114 * u64::from(pixel[0]) + 587 * u64::from(pixel[1]) + 299 * u64::from(pixel[2]))
                / 1000;
        covered += alpha;
        light += alpha * luma;
    }
    let backdrop = if covered > 0 && light / covered > 160 {
        0
    } else {
        255
    };
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * alpha + backdrop * (255 - alpha) + 127) / 255) as u8;
        }
        pixel[3] = 255;
    }
}

#[cfg(test)]
mod tests {
    use super::{Read, flatten, laid, pixel_rules, recognize};
    use serde_json::json;

    /// A white picture of `width` by `height` with the black lines `rules`
    /// (`[x0, y0, x1, y1]`, two pixels thick).
    fn ruled(width: usize, height: usize, rules: &[[usize; 4]]) -> Vec<u8> {
        let mut pixels = vec![255u8; width * height * 4];
        for [x0, y0, x1, y1] in rules {
            for y in *y0..=(*y1 + 1).min(height - 1) {
                for x in *x0..=(*x1 + 1).min(width - 1) {
                    pixels[(y * width + x) * 4..][..3].fill(0);
                }
            }
        }
        pixels
    }

    /// Words 20 pixels high at `(x, y)`, each a line of its own as the
    /// engine gives the cells of a table, 12 pixels wide a character.
    fn words(words: &[(&str, f64, f64)]) -> Vec<Vec<(String, [f64; 4])>> {
        words
            .iter()
            .map(|(text, x, y)| {
                vec![(
                    (*text).to_owned(),
                    [*x, *y, text.chars().count() as f64 * 12.0, 20.0],
                )]
            })
            .collect()
    }

    #[test]
    fn the_lines_a_picture_rules_are_found_and_filled_areas_and_text_are_not() {
        let mut pixels = ruled(400, 300, &[[20, 40, 380, 40], [200, 40, 200, 260]]);
        // A filled block, and strokes as short as those of letters.
        for y in 100..180 {
            for x in 30..150 {
                pixels[(y * 400 + x) * 4..][..3].fill(0);
            }
        }
        for x in 250..270 {
            pixels[(220 * 400 + x) * 4..][..3].fill(0);
        }
        let rules = pixel_rules(400, 300, &pixels, 20.0);
        assert!(!rules.is_empty());
        for rule in &rules {
            if rule.level {
                assert!((40.0..43.0).contains(&rule.at), "{rule:?}");
                assert!(rule.from <= 21.0 && rule.to >= 380.0, "{rule:?}");
            } else {
                assert!((200.0..203.0).contains(&rule.at), "{rule:?}");
                assert!(rule.from <= 41.0 && rule.to >= 260.0, "{rule:?}");
            }
        }
        assert!(rules.iter().any(|r| r.level) && rules.iter().any(|r| !r.level));
    }

    #[test]
    fn words_in_a_ruled_picture_are_read_as_the_cells_of_its_table() {
        // A table of three columns and rows under a heading, its words read
        // in another order than they stand.
        let mut lines = vec![];
        for y in [100, 150, 200, 250] {
            lines.push([40, y, 640, y]);
        }
        for x in [40, 240, 440, 640] {
            lines.push([x, 100, x, 250]);
        }
        let read = Read {
            language: "en-US".into(),
            width: 700,
            height: 300,
            lines: words(&[
                ("120", 460.0, 165.0),
                ("Parts", 40.0, 40.0),
                ("Code", 60.0, 115.0),
                ("Qty", 460.0, 115.0),
                ("Name", 260.0, 115.0),
                ("A-1", 60.0, 165.0),
                ("Hex", 260.0, 158.0),
                ("bolt", 310.0, 158.0),
                ("B-2", 60.0, 215.0),
                ("300", 460.0, 215.0),
            ]),
            angle: 0.0,
            pixels: ruled(700, 300, &lines),
        };
        let (text, layout) = laid(&read);
        assert_eq!(
            text,
            "Parts\nCode\tName\tQty\nA-1\tHex bolt\t120\nB-2\t\t300"
        );
        let layout = layout.unwrap();
        assert_eq!(layout["size"], json!([700, 300]));
        assert_eq!(
            layout["tables"],
            json!([{"name":"table-1","range":"A2:C4","totals_rows":0}])
        );
        assert_eq!(layout["merges"], json!([]));
        assert_eq!(
            layout["cells"][5],
            json!({"address":"B3","text":"Hex bolt","box":[260, 158, 98, 20]})
        );
        assert_eq!(layout["cells"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn words_of_a_picture_without_rulings_are_read_line_by_line() {
        // Japanese is read a character a word; no space stands between
        // them, and one does between words written with spaces.
        let mut lines = words(&[("order", 20.0, 60.0), ("no.", 90.0, 60.0)]);
        lines.insert(
            0,
            ["受", "注", "票"]
                .iter()
                .enumerate()
                .map(|(i, c)| ((*c).to_owned(), [20.0 + 24.0 * i as f64, 20.0, 20.0, 20.0]))
                .collect(),
        );
        let read = Read {
            language: "ja".into(),
            width: 300,
            height: 100,
            lines,
            angle: 0.0,
            pixels: ruled(300, 100, &[]),
        };
        let (text, layout) = laid(&read);
        assert_eq!(text, "受注票\norder no.");
        assert_eq!(layout.unwrap()["tables"], json!([]));
        let none = Read {
            lines: vec![],
            ..read
        };
        assert_eq!(laid(&none), (String::new(), None));
    }

    #[test]
    fn an_svg_picture_is_read_by_its_text_elements() {
        let svg = br#"<?xml version="1.0"?><!DOCTYPE svg><svg xmlns="http://www.w3.org/2000/svg"><title>Icon</title><defs><text id="t">hidden</text></defs><text x="1" y="2">ZQ100 <tspan>first</tspan></text><g><text>second
line</text></g></svg>"#;
        let read = recognize("image-001.svg", svg);
        assert_eq!(read["status"], "available");
        assert_eq!(read["text"], "ZQ100 first\nsecond line");
        // Known by its bytes too.
        assert_eq!(recognize("image-002.bin", svg)["text"], read["text"]);
        assert_eq!(recognize("image-003.svg", b"<svg")["status"], "unavailable");
    }

    #[test]
    fn transparent_pixels_take_the_backdrop_that_shows_the_picture() {
        // Black text on a transparent ground whose hidden color is black too.
        let mut dark = [0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 128];
        flatten(&mut dark);
        assert_eq!(dark, [0, 0, 0, 255, 255, 255, 255, 255, 127, 127, 127, 255]);
        // White text on a transparent ground goes over black.
        let mut light = [255, 255, 255, 255, 255, 255, 255, 0];
        flatten(&mut light);
        assert_eq!(light, [255, 255, 255, 255, 0, 0, 0, 255]);
        // An opaque picture is left alone.
        let mut opaque = [10, 20, 30, 255];
        flatten(&mut opaque);
        assert_eq!(opaque, [10, 20, 30, 255]);
    }
}
