//! Automatic OCR for imported image assets on Windows. The image is read as
//! 8-bit BGRA, made small enough for the engine, and laid over a backdrop
//! where it is transparent, since the engine reads the colors alone and a
//! transparent picture's hidden colors are often all black. A metafile (EMF,
//! WMF: a picture pasted from Excel, PowerPoint or Visio, the view of an
//! embedded object) is drawn on white by Windows first; an SVG picture's text
//! is read from its text elements, as it is text rather than pixels.
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
        Ok((text, language)) => json!({
            "status":"available",
            "text":text,
            "reason":format!("Windows OCR ({language}) executed on imported image")
        }),
        Err(error) => json!({
            "status":"unavailable",
            "text":"",
            "reason":format!("Windows OCR could not process imported image: {error:#}")
        }),
    }
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
fn run(_bytes: &[u8]) -> Result<(String, String)> {
    anyhow::bail!("Windows OCR requires Windows")
}

#[cfg(windows)]
fn run(bytes: &[u8]) -> Result<(String, String)> {
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
fn recognize_winrt(bytes: &[u8]) -> windows::core::Result<(String, String)> {
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
    if let Some((width, height, pixels)) = metafile_pixels(bytes)? {
        let writer = DataWriter::new()?;
        writer.WriteBytes(&pixels)?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &writer.DetachBuffer()?,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
        )?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        let text = result.Text()?.to_string();
        let language = engine.RecognizerLanguage()?.LanguageTag()?.to_string();
        bitmap.Close()?;
        return Ok((text, language));
    }
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
        let scale =
            |side: u32| (u64::from(side) * u64::from(limit) / u64::from(longest)).max(1) as u32;
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
    // An image without alpha (JPEG, a PNG without it) is read as it is.
    let bitmap = if decoder.BitmapAlphaMode()? == BitmapAlphaMode::Ignore {
        read
    } else {
        let (width, height) = (read.PixelWidth()?, read.PixelHeight()?);
        let size = (width as usize) * (height as usize) * 4;
        let buffer = Buffer::Create(size as u32)?;
        read.CopyToBuffer(&buffer)?;
        read.Close()?;
        let mut pixels = vec![0u8; size];
        DataReader::FromBuffer(&buffer)?.ReadBytes(&mut pixels)?;
        drop(buffer);
        flatten(&mut pixels);
        let writer = DataWriter::new()?;
        writer.WriteBytes(&pixels)?;
        drop(pixels);
        SoftwareBitmap::CreateCopyFromBuffer(
            &writer.DetachBuffer()?,
            BitmapPixelFormat::Bgra8,
            width,
            height,
        )?
    };
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let text = result.Text()?.to_string();
    let language = engine.RecognizerLanguage()?.LanguageTag()?.to_string();
    bitmap.Close()?;
    Ok((text, language))
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
    use super::{flatten, recognize};

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
