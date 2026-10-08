//! Removing rights management protection (IRM and sensitivity labels) from an
//! Office original. Its key is issued by the rights management service to the
//! signed-in user, so Office itself opens a copy in a disposable worker,
//! removes the protection and saves an unprotected copy. The account needs the
//! right to remove it (owner or export) and to access the content
//! programmatically.

use crate::document_source::{EXCEL_FORMATS, WORD_FORMATS, encryption};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

/// Seconds Office may take. A sign-in prompt it cannot show ends here.
const TIMEOUT: u64 = 180;
/// The justification Office records for removing a label.
const JUSTIFICATION: &str = "ARP removed the label from an original that must not be encrypted";
const PERMISSION_REFUSED: &str = "Office refused to remove the IRM permission; the signed-in account needs owner or export rights";
const LABEL_REFUSED: &str = "Office refused to remove the sensitivity label; the label policy or the signed-in account's rights do not allow it";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Application {
    Excel,
    Word,
    PowerPoint,
}

impl Application {
    fn of(format: &str) -> Result<Self> {
        Ok(if EXCEL_FORMATS.contains(&format) {
            Self::Excel
        } else if WORD_FORMATS.contains(&format) {
            Self::Word
        } else if format == "pptx" {
            Self::PowerPoint
        } else {
            bail!("protection is only removed from Excel, Word and PowerPoint files, not .{format}")
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Excel => "Excel",
            Self::Word => "Word",
            Self::PowerPoint => "PowerPoint",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Request {
    application: Application,
    source: String,
    output: String,
    result: String,
}

/// An original with its protection removed.
pub struct Unprotected {
    pub bytes: Vec<u8>,
    /// What Office removed: `permission` (IRM) and `sensitivity_label`.
    pub removed: Vec<String>,
}

/// The package of `original` without its rights management protection, removed
/// by Office in a worker that uses `stage` for its files. `original` is not
/// changed.
pub fn remove(original: &Path, stage: &Path) -> Result<Unprotected> {
    let format = original
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let application = Application::of(&format)?;
    let name = application.name();
    ensure!(
        cfg!(windows),
        "removing IRM or sensitivity label protection requires Windows with desktop {name} signed in to an account allowed to remove it"
    );
    let source = stage.join(format!("protected.{format}"));
    fs::copy(original, &source)?;
    let output = stage.join(format!("unprotected.{format}"));
    let result = stage.join("result.json");
    let request = stage.join("request.json");
    let text = |path: &Path| path.to_str().map(String::from).context("non-Unicode path");
    fs::write(
        &request,
        serde_json::to_vec(&Request {
            application,
            source: text(&source)?,
            output: text(&output)?,
            result: text(&result)?,
        })?,
    )?;
    let log = stage.join("stderr.txt");
    let status = crate::office_worker::run("--internal-office-unprotect", &request, &log, TIMEOUT)?
        .with_context(|| {
            format!(
                "{name} did not finish removing the protection within {TIMEOUT} seconds; it may be waiting for a sign-in. Open the file in {name} once, then import again"
            )
        })?;
    ensure!(
        status.success(),
        "{name} could not remove the protection: {}",
        String::from_utf8_lossy(&fs::read(&log)?).trim()
    );
    let bytes = fs::read(&output)?;
    ensure!(
        bytes.starts_with(b"PK\x03\x04") && encryption::protection(&bytes).is_none(),
        "{name} saved the file still protected; the signed-in account needs the right to remove the protection (owner or export)"
    );
    let removed = serde_json::from_slice(&fs::read(&result)?)?;
    Ok(Unprotected { bytes, removed })
}

/// The calls removing protection makes on a document open in Office.
trait Protected {
    /// Whether rights management restricts the document (`Permission.Enabled`).
    fn restricted(&self) -> Result<bool>;
    fn unrestrict(&self) -> Result<()>;
    /// The ID of the document's sensitivity label; `None` without a label or
    /// where Office has no label API.
    fn label(&self) -> Result<Option<String>>;
    fn remove_label(&self, justification: &str) -> Result<()>;
    fn save_as(&self, output: &Path) -> Result<()>;
}

/// Removes the IRM permission of `document`. A label applies its own
/// protection, so when the permission stays, or the document is protected
/// without one, its sensitivity label is removed too. Then the document is
/// saved to `output`. Returns what was removed.
fn unprotect(document: &impl Protected, output: &Path) -> Result<Vec<String>> {
    let mut removed = vec![];
    let restricted = document.restricted()?;
    let mut refusal = None;
    if restricted {
        match document.unrestrict() {
            Ok(()) => removed.push("permission".to_owned()),
            Err(error) => refusal = Some(error),
        }
    }
    if !restricted || document.restricted()? {
        let labelled = document.label()?.is_some();
        if labelled {
            document
                .remove_label(JUSTIFICATION)
                .context(LABEL_REFUSED)?;
            removed.push("sensitivity_label".to_owned());
        }
        if document.restricted()? {
            match refusal {
                Some(error) if !labelled => return Err(error.context(PERMISSION_REFUSED)),
                _ => {
                    document.unrestrict().context(PERMISSION_REFUSED)?;
                    removed.push("permission".to_owned());
                }
            }
        }
    }
    document.save_as(output)?;
    Ok(removed)
}

#[cfg(windows)]
pub use office::worker;

#[cfg(windows)]
mod office {
    use super::*;
    use crate::office_com::{Apartment, Arg, call, get, isolated_application, method, prop, put};
    use windows::Win32::System::Com::IDispatch;

    struct Document {
        application: Application,
        document: IDispatch,
    }

    // Office without rights management set up fails to read the permission or
    // the label; that shows no protection. The saved copy is checked anyway.
    impl Protected for Document {
        fn restricted(&self) -> Result<bool> {
            Ok(prop(&self.document, "Permission")
                .and_then(|permission| get(&permission, "Enabled")?.boolean())
                .unwrap_or(false))
        }

        fn unrestrict(&self) -> Result<()> {
            put(
                &prop(&self.document, "Permission")?,
                "Enabled",
                Arg::Bool(false),
            )
        }

        fn label(&self) -> Result<Option<String>> {
            // Office before the label API has no SensitivityLabel member.
            let id = prop(&self.document, "SensitivityLabel")
                .and_then(|labels| get(&method(&labels, "GetLabel", vec![])?, "LabelId")?.string())
                .unwrap_or_default();
            Ok((!id.is_empty()).then_some(id))
        }

        fn remove_label(&self, justification: &str) -> Result<()> {
            let labels = prop(&self.document, "SensitivityLabel")?;
            // An empty label removes the label; the context is returned to
            // LabelChanged handlers only.
            let empty = method(&labels, "CreateLabelInfo", vec![])?;
            put(&empty, "Justification", Arg::Text(justification))?;
            call(
                &labels,
                "SetLabel",
                vec![Arg::Object(&empty), Arg::Object(&empty)],
            )?;
            Ok(())
        }

        fn save_as(&self, output: &Path) -> Result<()> {
            let path = output.to_str().context("non-Unicode path")?;
            let document = &self.document;
            match self.application {
                Application::Excel => {
                    let format = get(document, "FileFormat")?.int()?;
                    call(document, "SaveAs", vec![Arg::Text(path), Arg::Int(format)])?;
                }
                Application::Word => {
                    let format = get(document, "SaveFormat")?.int()?;
                    call(document, "SaveAs2", vec![Arg::Text(path), Arg::Int(format)])?;
                }
                Application::PowerPoint => {
                    // ppSaveAsOpenXMLPresentation
                    call(document, "SaveAs", vec![Arg::Text(path), Arg::Int(24)])?;
                }
            }
            Ok(())
        }
    }

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
                let outcome = unprotect(
                    &Document {
                        application: request.application,
                        document: document.clone(),
                    },
                    Path::new(&request.output),
                );
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

    fn open(application: &IDispatch, kind: Application, source: &str) -> Result<IDispatch> {
        put(application, "AutomationSecurity", Arg::Int(3))?;
        match kind {
            Application::Excel => {
                put(application, "Visible", Arg::Bool(false))?;
                put(application, "DisplayAlerts", Arg::Bool(false))?;
                put(application, "EnableEvents", Arg::Bool(false))?;
                put(application, "AskToUpdateLinks", Arg::Bool(false))?;
                method(
                    &prop(application, "Workbooks")?,
                    "Open",
                    vec![
                        Arg::Text(source),
                        Arg::Int(0),
                        Arg::Bool(false),
                        Arg::Missing,
                        Arg::Text(""),
                        Arg::Text(""),
                        Arg::Bool(true),
                        Arg::Missing,
                        Arg::Missing,
                        Arg::Bool(false),
                        Arg::Bool(false),
                        Arg::Missing,
                        Arg::Bool(false),
                    ],
                )
            }
            Application::Word => {
                put(application, "Visible", Arg::Bool(false))?;
                // wdAlertsNone
                put(application, "DisplayAlerts", Arg::Int(0))?;
                method(
                    &prop(application, "Documents")?,
                    "Open",
                    vec![
                        Arg::Text(source),
                        Arg::Bool(false),
                        Arg::Bool(false),
                        Arg::Bool(false),
                        Arg::Text(""),
                        Arg::Text(""),
                        Arg::Bool(false),
                        Arg::Text(""),
                        Arg::Text(""),
                        Arg::Missing,
                        Arg::Missing,
                        Arg::Bool(false),
                    ],
                )
            }
            Application::PowerPoint => {
                // ppAlertsNone
                put(application, "DisplayAlerts", Arg::Int(1))?;
                // Read-write, titled, without a window.
                method(
                    &prop(application, "Presentations")?,
                    "Open",
                    vec![Arg::Text(source), Arg::Int(0), Arg::Int(0), Arg::Int(0)],
                )
            }
        }
    }

    fn close(document: &IDispatch, kind: Application) {
        let _ = match kind {
            Application::Excel => call(document, "Close", vec![Arg::Bool(false)]),
            // wdDoNotSaveChanges
            Application::Word => call(document, "Close", vec![Arg::Int(0)]),
            Application::PowerPoint => call(document, "Close", vec![]),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::cell::RefCell;

    /// An Office document whose protection comes from an IRM permission, a
    /// sensitivity label or both, recording the calls made on it.
    #[derive(Default)]
    struct Mock {
        permission: RefCell<bool>,
        /// The label applies the protection: removing it lifts the permission.
        label: RefCell<Option<&'static str>>,
        label_protects: bool,
        permission_refused: bool,
        label_refused: bool,
        calls: RefCell<Vec<String>>,
    }

    impl Protected for Mock {
        fn restricted(&self) -> Result<bool> {
            Ok(*self.permission.borrow())
        }
        fn unrestrict(&self) -> Result<()> {
            self.calls.borrow_mut().push("unrestrict".into());
            if self.permission_refused || (self.label_protects && self.label.borrow().is_some()) {
                return Err(anyhow!("Office COM call \"Enabled\" failed"));
            }
            *self.permission.borrow_mut() = false;
            Ok(())
        }
        fn label(&self) -> Result<Option<String>> {
            Ok(self.label.borrow().map(String::from))
        }
        fn remove_label(&self, justification: &str) -> Result<()> {
            assert_eq!(justification, JUSTIFICATION);
            self.calls.borrow_mut().push("remove_label".into());
            if self.label_refused {
                return Err(anyhow!("Office COM call \"SetLabel\" failed"));
            }
            *self.label.borrow_mut() = None;
            if self.label_protects {
                *self.permission.borrow_mut() = false;
            }
            Ok(())
        }
        fn save_as(&self, output: &Path) -> Result<()> {
            assert_eq!(output, Path::new("out.docx"));
            self.calls.borrow_mut().push("save".into());
            Ok(())
        }
    }

    fn run(mock: &Mock) -> (Result<Vec<String>>, Vec<String>) {
        let outcome = unprotect(mock, Path::new("out.docx"));
        (outcome, mock.calls.borrow().clone())
    }

    #[test]
    fn irm_permission_is_removed_and_a_label_kept() {
        let mock = Mock {
            permission: RefCell::new(true),
            label: RefCell::new(Some("general")),
            ..Mock::default()
        };
        let (removed, calls) = run(&mock);
        assert_eq!(removed.unwrap(), ["permission"]);
        assert_eq!(calls, ["unrestrict", "save"]);
        assert_eq!(*mock.label.borrow(), Some("general"));
    }

    #[test]
    fn a_protecting_label_is_removed_when_the_permission_stays() {
        let mock = Mock {
            permission: RefCell::new(true),
            label: RefCell::new(Some("confidential")),
            label_protects: true,
            ..Mock::default()
        };
        let (removed, calls) = run(&mock);
        assert_eq!(removed.unwrap(), ["sensitivity_label"]);
        assert_eq!(calls, ["unrestrict", "remove_label", "save"]);
    }

    #[test]
    fn a_label_is_removed_when_office_shows_no_permission() {
        let mock = Mock {
            label: RefCell::new(Some("confidential")),
            ..Mock::default()
        };
        let (removed, calls) = run(&mock);
        assert_eq!(removed.unwrap(), ["sensitivity_label"]);
        assert_eq!(calls, ["remove_label", "save"]);
    }

    #[test]
    fn a_label_policy_refusal_saves_nothing() {
        let mock = Mock {
            permission: RefCell::new(true),
            label: RefCell::new(Some("confidential")),
            label_protects: true,
            label_refused: true,
            ..Mock::default()
        };
        let (removed, calls) = run(&mock);
        let error = format!("{:#}", removed.unwrap_err());
        assert!(error.contains("label policy"), "{error}");
        assert_eq!(calls, ["unrestrict", "remove_label"]);
    }

    #[test]
    fn missing_rights_to_remove_the_permission_save_nothing() {
        let mock = Mock {
            permission: RefCell::new(true),
            permission_refused: true,
            ..Mock::default()
        };
        let (removed, calls) = run(&mock);
        let error = format!("{:#}", removed.unwrap_err());
        assert!(error.contains("owner or export"), "{error}");
        assert!(error.contains("Enabled"), "{error}");
        assert_eq!(calls, ["unrestrict"]);
    }

    #[test]
    fn formats_choose_their_office_application() {
        assert_eq!(Application::of("xltm").unwrap(), Application::Excel);
        assert_eq!(Application::of("dotx").unwrap(), Application::Word);
        assert_eq!(Application::of("pptx").unwrap(), Application::PowerPoint);
        assert!(Application::of("pdf").is_err());
    }
}
