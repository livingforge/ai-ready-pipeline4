//! Converting a Word or PowerPoint document to PDF with desktop Office in a
//! disposable worker, so that its pages are rendered as Windows renders PDF.

use crate::rights_management::Application;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Serialize, Deserialize)]
struct Request {
    application: Application,
    source: String,
    output: String,
}

/// The PDF Word or PowerPoint saves of `document`, converted in a worker that
/// uses `stage` for its files. `document` is not changed.
pub fn convert(document: &Path, stage: &Path, timeout: u64) -> Result<Vec<u8>> {
    let format = document
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let application = Application::of(&format)?;
    ensure!(
        application != Application::Excel,
        "Excel sheets are rendered by Excel, not as PDF"
    );
    let name = application.name();
    ensure!(
        cfg!(windows),
        "rendering a {name} document requires Windows and desktop {name}"
    );
    let source = stage.join(format!("document.{format}"));
    fs::copy(document, &source)?;
    let output = stage.join("document.pdf");
    let request = stage.join("request.json");
    let text = |path: &Path| path.to_str().map(String::from).context("non-Unicode path");
    fs::write(
        &request,
        serde_json::to_vec(&Request {
            application,
            source: text(&source)?,
            output: text(&output)?,
        })?,
    )?;
    let log = stage.join("stderr.txt");
    let status = crate::office_worker::run("--internal-office-pdf", &request, &log, timeout)?
        .with_context(|| format!("{name} did not save the PDF within {timeout} seconds"))?;
    ensure!(
        status.success(),
        "{name} could not save the document as PDF: {}",
        String::from_utf8_lossy(&fs::read(&log)?).trim()
    );
    let bytes = fs::read(&output)?;
    ensure!(bytes.starts_with(b"%PDF-"), "{name} saved no PDF");
    Ok(bytes)
}

#[cfg(windows)]
pub use office::worker;

#[cfg(windows)]
mod office {
    use super::*;
    use crate::office_com::{Apartment, Arg, call, get, isolated_application, method, prop, put};
    use crate::rights_management::office::{close, open};
    use windows::Win32::System::Com::IDispatch;

    pub fn worker(request_path: &Path) -> Result<()> {
        let request: Request = serde_json::from_slice(&fs::read(request_path)?)?;
        let _apartment = Apartment::new()?;
        let (prog_id, executable) = match request.application {
            Application::Word => ("Word.Application", "winword.exe"),
            Application::PowerPoint => ("PowerPoint.Application", "powerpnt.exe"),
            Application::Excel => anyhow::bail!("Excel sheets are rendered by Excel"),
        };
        let (application, job) = isolated_application(prog_id, executable)?;
        let outcome =
            open(&application, request.application, &request.source).and_then(|document| {
                let output = Arg::Text(&request.output);
                let saved = match request.application {
                    // wdExportFormatPDF
                    Application::Word => {
                        call(&document, "ExportAsFixedFormat", vec![output, Arg::Int(17)])
                    }
                    // Every slide, hidden ones too: the disposable copy shows
                    // them all before ppSaveAsPDF, which leaves hidden ones out.
                    _ => show_slides(&document)
                        .and_then(|()| call(&document, "SaveAs", vec![output, Arg::Int(32)])),
                };
                close(&document, request.application);
                saved.map(|_| ())
            });
        // A stuck COM call is ended by the parent killing this worker, which
        // closes the job handle and its Office process.
        let _ = call(&application, "Quit", vec![]);
        drop(job);
        outcome
    }

    /// Shows every slide of the presentation `document` in the slide show.
    fn show_slides(document: &IDispatch) -> Result<()> {
        let slides = prop(document, "Slides")?;
        for index in 1..=get(&slides, "Count")?.int()? {
            let slide = method(&slides, "Item", vec![Arg::Int(index)])?;
            // msoFalse
            put(&prop(&slide, "SlideShowTransition")?, "Hidden", Arg::Int(0))?;
        }
        Ok(())
    }
}
