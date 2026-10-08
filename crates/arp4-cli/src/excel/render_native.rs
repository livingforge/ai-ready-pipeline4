//! Excel's late-bound COM automation lives in a disposable STA process.
#![cfg(windows)]

use crate::office_com::{
    Apartment, Arg, Handle, call, get, invoke, isolated_application, item, method, prop, put,
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::json;
use std::{fs, path::Path};
use windows::{
    Win32::{
        Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0},
        System::{
            Com::{DISPATCH_PROPERTYGET, IDispatch},
            Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        },
    },
    core::w,
};

#[derive(Deserialize)]
struct Request {
    source: String,
    sheet: String,
    range: Option<String>,
    output: String,
    result: String,
}

struct RenderMutex(Handle);
impl RenderMutex {
    fn lock() -> Result<Self> {
        let handle = Handle(unsafe { CreateMutexW(None, false, w!("Local\\ARP4ExcelRender"))? });
        let state = unsafe { WaitForSingleObject(handle.0, 0) };
        ensure!(
            state == WAIT_OBJECT_0 || state == WAIT_ABANDONED,
            "Another Excel render is using the clipboard; retry after it finishes."
        );
        Ok(Self(handle))
    }
}
impl Drop for RenderMutex {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0.0);
        }
    }
}

fn address(mut column: i32, row: i32) -> String {
    let mut letters = String::new();
    while column > 0 {
        letters.insert(0, (b'A' + ((column - 1) % 26) as u8) as char);
        column = (column - 1) / 26;
    }
    format!("{letters}{row}")
}

fn png_dimensions(path: &Path) -> Result<(u32, u32)> {
    let bytes = fs::read(path)?;
    ensure!(
        bytes.len() >= 24
            && bytes[..8] == [137, 80, 78, 71, 13, 10, 26, 10]
            && &bytes[12..16] == b"IHDR",
        "Invalid rendered image"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    ensure!(
        width > 1
            && height > 1
            && width <= 8192
            && height <= 8192
            && u64::from(width) * u64::from(height) <= 32_000_000,
        "Invalid or oversized rendered image"
    );
    Ok((width, height))
}

pub fn worker(request_path: &Path) -> Result<()> {
    let request: Request = serde_json::from_slice(&fs::read(request_path)?)?;
    super::render::validate_source(Path::new(&request.source))?;
    let _mutex = RenderMutex::lock()?;
    let _apartment = Apartment::new()?;
    let (excel, job) = isolated_application("Excel.Application", "excel.exe")?;
    let outcome = render_with_excel(&excel, &request);
    // Drop the job after a best-effort graceful close; a stuck COM call is handled by
    // the parent killing this worker, which closes the job handle and its Excel.
    let _ = call(&excel, "Quit", vec![]);
    drop(job);
    outcome
}

fn render_with_excel(excel: &IDispatch, request: &Request) -> Result<()> {
    put(excel, "Visible", Arg::Bool(false))?;
    put(excel, "DisplayAlerts", Arg::Bool(false))?;
    put(excel, "EnableEvents", Arg::Bool(false))?;
    put(excel, "AskToUpdateLinks", Arg::Bool(false))?;
    put(excel, "CopyObjectsWithCells", Arg::Bool(true))?;
    put(excel, "AutomationSecurity", Arg::Int(3))?;
    let workbooks = prop(excel, "Workbooks")?;
    let scratch = method(&workbooks, "Add", vec![])?;
    put(excel, "Calculation", Arg::Int(-4135))?;
    let source = method(
        &workbooks,
        "Open",
        vec![
            Arg::Text(&request.source),
            Arg::Int(0),
            Arg::Bool(true),
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
    )?;
    let outcome = render_range(excel, &scratch, &source, request);
    let _ = call(&source, "Close", vec![Arg::Bool(false)]);
    let _ = call(&scratch, "Close", vec![Arg::Bool(false)]);
    outcome
}

fn render_range(
    excel: &IDispatch,
    scratch: &IDispatch,
    source: &IDispatch,
    request: &Request,
) -> Result<()> {
    let sheets = prop(source, "Worksheets")?;
    let sheet = item(&sheets, Arg::Text(&request.sheet))?;
    ensure!(
        get(&sheet, "Visible")?.int()? == -1,
        "The requested sheet is hidden. Render a visible sheet; hidden state is not changed."
    );
    call(&sheet, "Activate", vec![])?;
    let cells = match &request.range {
        Some(range) if !range.is_empty() => invoke(
            &sheet,
            "Range",
            DISPATCH_PROPERTYGET,
            vec![Arg::Text(range)],
        )?
        .dispatch()?,
        _ => prop(&sheet, "UsedRange")?,
    };
    let column = get(&cells, "Column")?.int()?;
    let row = get(&cells, "Row")?.int()?;
    let last_column = column + get(&prop(&cells, "Columns")?, "Count")?.int()? - 1;
    let last_row = row + get(&prop(&cells, "Rows")?, "Count")?.int()? - 1;
    let first = address(column, row);
    let last = address(last_column, last_row);
    let range = if first == last {
        first
    } else {
        format!("{first}:{last}")
    };
    let width = get(&cells, "Width")?.float()?;
    let height = get(&cells, "Height")?.float()?;
    ensure!(
        width > 0.0 && height > 0.0,
        "The requested range has no visible area."
    );
    ensure!(
        width * 4.0 / 3.0 <= 8192.0
            && height * 4.0 / 3.0 <= 8192.0
            && width * height * 16.0 / 9.0 <= 32_000_000.0,
        "Range exceeds the render budget; request smaller explicit cell ranges."
    );
    let scratch_sheet = item(&prop(scratch, "Worksheets")?, Arg::Int(1))?;
    let chart_objects = method(&scratch_sheet, "ChartObjects", vec![])?;
    let chart_object = method(
        &chart_objects,
        "Add",
        vec![
            Arg::Int(0),
            Arg::Int(0),
            Arg::Float(width),
            Arg::Float(height),
        ],
    )?;
    let chart = prop(&chart_object, "Chart")?;
    let line = prop(&prop(&prop(&chart, "ChartArea")?, "Format")?, "Line")?;
    put(&line, "Visible", Arg::Int(0))?;
    call(&sheet, "Activate", vec![])?;
    call(&cells, "CopyPicture", vec![Arg::Int(1), Arg::Int(-4147)])?;
    call(scratch, "Activate", vec![])?;
    call(&chart_object, "Activate", vec![])?;
    call(&chart, "Paste", vec![])?;
    ensure!(
        call(
            &chart,
            "Export",
            vec![
                Arg::Text(&request.output),
                Arg::Text("PNG"),
                Arg::Bool(false)
            ]
        )?
        .boolean()?,
        "Excel PNG export failed."
    );
    let (width, height) = png_dimensions(Path::new(&request.output))?;
    let version = get(excel, "Version")?.string()?;
    fs::write(
        &request.result,
        serde_json::to_vec(&json!({
            "engine":"excel-com", "version":version, "sheet":request.sheet,
            "range":range, "width":width, "height":height, "clipboard_changed":true,
        }))?,
    )?;
    Ok(())
}
