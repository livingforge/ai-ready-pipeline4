//! Native Excel rendering, isolated from the scalar/structural writeback engine.
use crate::data::{Rejection, encoded, hash, read};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

/// Renders `range` of `sheet` of the workbook `source` to PNG with desktop
/// Excel. With `recalculate`, the formulas are calculated first, as Excel
/// does opening a workbook whose saved results are stale; otherwise the
/// results the workbook holds are drawn.
pub fn render(
    source: &Path,
    sheet: &str,
    range: Option<&str>,
    timeout: u64,
    recalculate: bool,
) -> Result<(Vec<u8>, Value)> {
    ensure!(
        cfg!(windows),
        "Excel rendering requires Windows and desktop Microsoft Excel"
    );
    ensure!(
        (1..=600).contains(&timeout),
        "render timeout must be 1..600 seconds"
    );
    let original = validate_source(source)?;
    let stage = tempfile::tempdir()?;
    let request_path = stage.path().join("request.json");
    let result_path = stage.path().join("result.json");
    let output = stage.path().join("render.png");
    let failure_path = stage.path().join("failure.json");
    // Waiting for another render takes at most half the time, leaving the
    // rest to this one.
    fs::write(
        &request_path,
        encoded(
            &json!({"source":source,"sheet":sheet,"range":range,"output":output,"result":result_path,"failure":failure_path,"lock_wait_seconds":timeout / 2,"recalculate":recalculate}),
        ),
    )?;
    let log = stage.path().join("stderr.txt");
    let Some(status) =
        crate::office_worker::run("--internal-excel-render", &request_path, &log, timeout)?
    else {
        return Err(Rejection(json!({
            "code":"office_render_timeout",
            "message":format!("Excel render did not finish in {timeout} seconds; no image or region was adopted"),
            "sheet":sheet,"requested_range":range,"timeout_seconds":timeout,
            "recovery":"A COM call stopped or a dialog waited. Retry with a longer --timeout or a smaller --range; if it stops again, open the workbook once in desktop Excel to clear any prompt (Protected View, recovery, activation).",
        }))
        .into());
    };
    ensure!(
        hash(&fs::read(source)?) == hash(&original),
        "original changed during rendering; output discarded"
    );
    if !status.success() {
        let failure = fs::read(&failure_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let log = String::from_utf8_lossy(&fs::read(log)?).trim().to_string();
        return Err(Rejection(failed(failure, &log, sheet, range)).into());
    }
    let result = read(&result_path, None)?;
    ensure!(
        result["engine"] == "excel-com" && result["sheet"] == sheet,
        "unexpected renderer result"
    );
    let bytes = fs::read(output)?;
    super::validate_image_asset("assets/render.png", &bytes)?;
    ensure!(
        bytes.len() >= 33 && &bytes[12..16] == b"IHDR",
        "invalid rendered PNG"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    ensure!(
        result["width"] == width && result["height"] == height && width > 1 && height > 1,
        "render dimensions mismatch"
    );
    Ok((bytes, result))
}

/// The rejection for a render the worker reported as failed: the step, the COM
/// error code, the clipboard tries, the tiles for a range over the budget, and
/// how to recover. Without the worker's report, its standard error is the message.
fn failed(failure: Option<Value>, log: &str, sheet: &str, range: Option<&str>) -> Value {
    let Some(mut failure) = failure else {
        return json!({"code":"office_render_failed","message":format!("Excel render failed: {log}"),"sheet":sheet,
            "requested_range":range,"stage":Value::Null,"recovery":recovery(None, None)});
    };
    let stage = failure["stage"].as_str().map(str::to_owned);
    let budget = stage.as_deref() == Some("budget");
    failure["code"] = json!(if budget {
        "render_budget_exceeded"
    } else {
        "office_render_failed"
    });
    failure["recovery"] = json!(recovery(stage.as_deref(), failure["hresult"].as_str()));
    failure
}

/// How to recover from a failed step of a render.
fn recovery(stage: Option<&str>, hresult: Option<&str>) -> &'static str {
    match (stage, hresult) {
        (Some("budget"), _) => {
            "Render each range of `tiles` with its own --range, --id and --image; together they show the whole range at full size. When `tiles` is empty, `tiles_needed` is how many images the range would take (0: one row or column alone is too large): give a narrower range, such as the cells the drawing covers."
        }
        (Some("copy_picture" | "paste"), Some("0x800A03EC")) => {
            "Another program kept the clipboard open through every try: close clipboard history, sync or monitoring tools, then retry."
        }
        (Some("clipboard_lock"), _) => {
            "Another ARP render holds the clipboard; retry after it finishes."
        }
        (Some("launch"), _) => {
            "Desktop Excel did not start: check that it is installed and licensed, that no Excel dialog (activation, safe mode, recovery) is waiting, and that the session is an interactive desktop."
        }
        (Some("open"), _) => {
            "Excel did not open the workbook: a blocked or protected file (Protected View, File Block settings, a password) or a damaged file. Open it once in desktop Excel to see why."
        }
        (Some("sheet"), _) => {
            "The sheet is missing or hidden; render a visible sheet by its exact name."
        }
        (Some("range"), _) => {
            "The range is not a valid range with visible cells on this sheet; give an A1 range such as A1:H30."
        }
        (Some("export" | "image" | "calibrate"), _) => {
            "Excel did not write a valid PNG; retry, or render a smaller range."
        }
        (Some("source"), _) => {
            "The workbook cannot be opened in Excel safely (Excel 4.0 macros or data connections)."
        }
        _ => "Retry once; if it fails again, render a smaller range and report the `message`.",
    }
}

pub(super) fn validate_source(source: &Path) -> Result<Vec<u8>> {
    let original = fs::read(source)?;
    let mut archive = crate::document_source::office_archive(&original)?;
    // These can execute or refresh on open independently of VBA event suppression.
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name().to_ascii_lowercase();
        ensure!(
            !name.starts_with("xl/macrosheets/") && name != "xl/connections.xml",
            "opening workbooks with Excel 4.0 macros or data connections in Excel is unsupported"
        );
    }
    Ok(original)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_render_says_its_step_code_and_how_to_recover() {
        let reported = json!({"stage":"copy_picture","hresult":"0x800A03EC","attempts":5,"message":"m","sheet":"S","requested_range":"A1:B2"});
        let rejection = failed(Some(reported), "", "S", Some("A1:B2"));
        assert_eq!(rejection["code"], "office_render_failed");
        assert_eq!(rejection["attempts"], 5);
        assert!(
            rejection["recovery"]
                .as_str()
                .unwrap()
                .contains("clipboard")
        );

        let over = json!({"stage":"budget","tiles":["A1:M40","N1:Z40"],"range":"A1:Z40"});
        let rejection = failed(Some(over), "", "S", None);
        assert_eq!(rejection["code"], "render_budget_exceeded");
        assert_eq!(rejection["tiles"], json!(["A1:M40", "N1:Z40"]));
        assert!(rejection["recovery"].as_str().unwrap().contains("tiles"));

        // A worker that died before reporting leaves its standard error as the message.
        let rejection = failed(None, "Excel render failed: no request", "S", None);
        assert_eq!(rejection["code"], "office_render_failed");
        assert!(
            rejection["message"]
                .as_str()
                .unwrap()
                .contains("no request")
        );
        assert_eq!(rejection["stage"], Value::Null);
    }
}
