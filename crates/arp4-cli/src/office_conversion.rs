//! Converting a binary Office original (.xls, .doc, .ppt and their kin) to the
//! Open XML format ARP reads. ARP does not parse the binary formats, so desktop
//! Office opens a copy in a disposable worker and saves it as Open XML: the
//! macro-enabled format when the file holds macros, which the other would drop.

use crate::rights_management::Application;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

/// Seconds Office may take. A prompt it cannot show ends here.
const TIMEOUT: u64 = 180;

/// A binary Office original saved as Open XML.
pub struct Converted {
    pub bytes: Vec<u8>,
    /// The extension of the Open XML format, lowercase.
    pub format: String,
    /// Whether the original holds macros, so the format is macro-enabled.
    pub macros: bool,
}

/// The application that opens the binary Office format `format`, a lowercase
/// extension; `None` for any other format.
pub(crate) fn application(format: &str) -> Option<Application> {
    match format {
        "xls" | "xlsb" | "xlt" => Some(Application::Excel),
        "doc" | "dot" => Some(Application::Word),
        "ppt" | "pot" | "pps" => Some(Application::PowerPoint),
        _ => None,
    }
}

/// The Open XML format the binary format `format` is saved as, with or
/// without `macros`, and the number its application names that format by. A
/// template stays a template; a slide show (.pps) becomes a presentation.
fn target(format: &str, macros: bool) -> Option<(&'static str, i32)> {
    Some(match (format, macros) {
        // xlOpenXMLWorkbook, xlOpenXMLWorkbookMacroEnabled
        ("xls" | "xlsb", false) => ("xlsx", 51),
        ("xls" | "xlsb", true) => ("xlsm", 52),
        // xlOpenXMLTemplate, xlOpenXMLTemplateMacroEnabled
        ("xlt", false) => ("xltx", 54),
        ("xlt", true) => ("xltm", 53),
        // wdFormatXMLDocument, wdFormatXMLDocumentMacroEnabled
        ("doc", false) => ("docx", 12),
        ("doc", true) => ("docm", 13),
        // wdFormatXMLTemplate, wdFormatXMLTemplateMacroEnabled
        ("dot", false) => ("dotx", 14),
        ("dot", true) => ("dotm", 15),
        // ppSaveAsOpenXMLPresentation, ppSaveAsOpenXMLPresentationMacroEnabled
        ("ppt" | "pps", false) => ("pptx", 24),
        ("ppt" | "pps", true) => ("pptm", 25),
        // ppSaveAsOpenXMLTemplate, ppSaveAsOpenXMLTemplateMacroEnabled
        ("pot", false) => ("potx", 26),
        ("pot", true) => ("potm", 27),
        _ => return None,
    })
}

#[derive(Serialize, Deserialize)]
struct Request {
    application: Application,
    /// The binary format of `source`.
    format: String,
    source: String,
    /// The path Office saves to, without the extension the worker chooses.
    output: String,
    result: String,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    format: String,
    macros: bool,
}

/// The Open XML package desktop Office saves of the binary Office file
/// `original`, converted in a worker that uses `stage` for its files.
/// `original` is not changed.
pub fn convert(original: &Path, stage: &Path) -> Result<Converted> {
    let format = original
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let application =
        application(&format).with_context(|| format!(".{format} is not a binary Office format"))?;
    let name = application.name();
    ensure!(
        cfg!(windows),
        "converting a binary Office file requires Windows and desktop {name}; elsewhere save it in Office as an Open XML file"
    );
    let source = stage.join(format!("original.{format}"));
    fs::copy(original, &source)?;
    let output = stage.join("converted");
    let result = stage.join("result.json");
    let request = stage.join("request.json");
    let text = |path: &Path| path.to_str().map(String::from).context("non-Unicode path");
    fs::write(
        &request,
        serde_json::to_vec(&Request {
            application,
            format,
            source: text(&source)?,
            output: text(&output)?,
            result: text(&result)?,
        })?,
    )?;
    let log = stage.join("stderr.txt");
    let status = crate::office_worker::run("--internal-office-convert", &request, &log, TIMEOUT)?
        .with_context(|| {
            format!(
                "{name} did not finish converting the file within {TIMEOUT} seconds; it may be waiting for a password or a prompt. Open the file in {name} once, then convert again"
            )
        })?;
    ensure!(
        status.success(),
        "{name} could not convert the file: {}",
        String::from_utf8_lossy(&fs::read(&log)?).trim()
    );
    let saved: Saved = serde_json::from_slice(&fs::read(&result)?)?;
    let bytes = fs::read(output.with_extension(&saved.format))?;
    ensure!(
        bytes.starts_with(b"PK\x03\x04"),
        "{name} saved no Open XML file"
    );
    Ok(Converted {
        bytes,
        format: saved.format,
        macros: saved.macros,
    })
}

#[cfg(windows)]
pub use office::worker;

#[cfg(windows)]
mod office {
    use super::*;
    use crate::office_com::{Apartment, Arg, call, get, isolated_application, prop};
    use crate::rights_management::office::{close, open};
    use windows::Win32::System::Com::IDispatch;

    pub fn worker(request_path: &Path) -> Result<()> {
        let request: Request = serde_json::from_slice(&fs::read(request_path)?)?;
        let _apartment = Apartment::new()?;
        let (prog_id, executable) = match request.application {
            Application::Excel => ("Excel.Application", "excel.exe"),
            Application::Word => ("Word.Application", "winword.exe"),
            Application::PowerPoint => ("PowerPoint.Application", "powerpnt.exe"),
        };
        let (application, job) = isolated_application(prog_id, executable)?;
        let outcome =
            open(&application, request.application, &request.source).and_then(|document| {
                let outcome = save(&document, &request);
                close(&document, request.application);
                outcome
            });
        // A stuck COM call is ended by the parent killing this worker, which
        // closes the job handle and its Office process.
        let _ = call(&application, "Quit", vec![]);
        drop(job);
        fs::write(&request.result, serde_json::to_vec(&outcome?)?)?;
        Ok(())
    }

    /// Saves the open `document` as the Open XML format its macros call for.
    fn save(document: &IDispatch, request: &Request) -> Result<Saved> {
        let macros = macros(document, request.application)
            .context("Office did not tell whether the file holds macros")?;
        let (format, number) =
            target(&request.format, macros).context("not a binary Office format")?;
        let output = format!("{}.{format}", request.output);
        let method = match request.application {
            Application::Word => "SaveAs2",
            _ => "SaveAs",
        };
        call(document, method, vec![Arg::Text(&output), Arg::Int(number)])?;
        Ok(Saved {
            format: format.to_owned(),
            macros,
        })
    }

    /// Whether `document` holds what only a macro-enabled format keeps: a VBA
    /// project, or in a workbook an Excel 4.0 macro sheet.
    fn macros(document: &IDispatch, application: Application) -> Result<bool> {
        let project = get(document, "HasVBProject")?;
        // PowerPoint may answer with msoTrue (-1) in place of a Boolean.
        if project
            .boolean()
            .or_else(|_| project.int().map(|n| n != 0))?
        {
            return Ok(true);
        }
        if application != Application::Excel {
            return Ok(false);
        }
        for sheets in ["Excel4MacroSheets", "Excel4IntlMacroSheets"] {
            if get(&prop(document, sheets)?, "Count")?.int()? > 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macros_choose_the_macro_enabled_format() {
        assert_eq!(target("xls", false), Some(("xlsx", 51)));
        assert_eq!(target("xls", true), Some(("xlsm", 52)));
        assert_eq!(target("xlsb", true), Some(("xlsm", 52)));
        assert_eq!(target("doc", false), Some(("docx", 12)));
        assert_eq!(target("doc", true), Some(("docm", 13)));
        assert_eq!(target("ppt", true), Some(("pptm", 25)));
        assert_eq!(target("pps", false), Some(("pptx", 24)));
    }

    #[test]
    fn templates_stay_templates() {
        assert_eq!(target("xlt", false), Some(("xltx", 54)));
        assert_eq!(target("xlt", true), Some(("xltm", 53)));
        assert_eq!(target("dot", true), Some(("dotm", 15)));
        assert_eq!(target("pot", false), Some(("potx", 26)));
    }

    #[test]
    fn every_binary_format_has_an_application_and_a_format_arp_reads() {
        for format in ["xls", "xlsb", "xlt", "doc", "dot", "ppt", "pot", "pps"] {
            assert!(
                crate::document_source::binary_office_replacement(format).is_some(),
                "{format}"
            );
            let application = application(format).unwrap();
            for macros in [false, true] {
                let (saved, _) = target(format, macros).unwrap();
                assert_eq!(Application::of(saved).unwrap(), application, "{format}");
            }
        }
        assert!(application("xlsx").is_none());
        assert!(target("xlsx", false).is_none());
    }
}
