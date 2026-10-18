//! Rendering a page of a PDF to a PNG image, as Windows renders PDF.
use anyhow::Result;

/// A rendered page: the PNG image and the number of pages of the PDF.
pub struct RenderedPage {
    pub png: Vec<u8>,
    pub pages: u32,
}

/// The `page` of `pdf`, from 1, rendered at twice its size in points.
#[cfg(not(windows))]
pub fn render(_pdf: &[u8], _page: u32, _timeout: u64) -> Result<RenderedPage> {
    anyhow::bail!("rendering PDF pages requires Windows")
}

/// The `page` of `pdf`, from 1, rendered at twice its size in points.
#[cfg(windows)]
pub fn render(pdf: &[u8], page: u32, timeout: u64) -> Result<RenderedPage> {
    use std::{sync::mpsc, time::Duration};
    let pdf = pdf.to_vec();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(render_winrt(&pdf, page));
    });
    receiver
        .recv_timeout(Duration::from_secs(timeout))
        .map_err(|_| anyhow::anyhow!("rendering the PDF page timed out after {timeout} seconds"))?
}

#[cfg(windows)]
fn render_winrt(pdf: &[u8], page: u32) -> Result<RenderedPage> {
    use windows::{
        Data::Pdf::{PdfDocument, PdfPageRenderOptions},
        Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream},
    };
    let input = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&input)?;
    writer.WriteBytes(pdf)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    writer.DetachStream()?;
    input.Seek(0)?;
    let document = PdfDocument::LoadFromStreamAsync(&input)?.join()?;
    let pages = document.PageCount()?;
    anyhow::ensure!(
        (1..=pages).contains(&page),
        "page {page} is outside the {pages} page(s) of the document"
    );
    let pdf_page = document.GetPage(page - 1)?;
    let size = pdf_page.Size()?;
    let options = PdfPageRenderOptions::new()?;
    // Twice the size in points keeps small text legible.
    options.SetDestinationWidth((size.Width * 2.0).round().max(1.0) as u32)?;
    let output = InMemoryRandomAccessStream::new()?;
    pdf_page
        .RenderWithOptionsToStreamAsync(&output, &options)?
        .join()?;
    pdf_page.Close()?;
    let length = u32::try_from(output.Size()?)?;
    let reader = DataReader::CreateDataReader(&output.GetInputStreamAt(0)?)?;
    reader.LoadAsync(length)?.join()?;
    let mut png = vec![0; length as usize];
    reader.ReadBytes(&mut png)?;
    Ok(RenderedPage { png, pages })
}
