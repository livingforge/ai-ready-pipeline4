//! Excel's late-bound COM automation lives in a disposable STA process.
#![cfg(windows)]

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashSet, fs, mem::ManuallyDrop, path::Path};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, WAIT_ABANDONED, WAIT_OBJECT_0},
        System::{
            Com::{
                CLSCTX_LOCAL_SERVER, CLSIDFromProgID, COINIT_APARTMENTTHREADED, CoCreateInstance,
                CoInitializeEx, CoUninitialize, DISPATCH_FLAGS, DISPATCH_METHOD,
                DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, IDispatch,
            },
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{
                CreateMutexW, OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE, ReleaseMutex,
                TerminateProcess, WaitForSingleObject,
            },
            Variant::{
                VARIANT, VT_BOOL, VT_BSTR, VT_DISPATCH, VT_ERROR, VT_I4, VT_R8, VariantClear,
            },
        },
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    },
    core::{BSTR, GUID, PCWSTR, w},
};

#[derive(Deserialize)]
struct Request {
    source: String,
    sheet: String,
    range: Option<String>,
    output: String,
    result: String,
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
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

fn excel_processes() -> Result<HashSet<u32>> {
    let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)? });
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut ids = HashSet::new();
    unsafe { Process32FirstW(snapshot.0, &mut entry) }
        .context("Cannot enumerate existing Excel processes")?;
    loop {
        let end = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case("excel.exe") {
            ids.insert(entry.th32ProcessID);
        }
        if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
            break;
        }
    }
    Ok(ids)
}

fn own_excel(id: u32) -> Result<Handle> {
    let job = Handle(unsafe { CreateJobObjectW(None, PCWSTR::null())? });
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )?;
        let process = Handle(OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE,
            false,
            id,
        )?);
        AssignProcessToJobObject(job.0, process.0).context("Cannot isolate Excel renderer")?;
    }
    Ok(job)
}

enum Arg<'a> {
    Int(i32),
    Float(f64),
    Bool(bool),
    Text(&'a str),
    Missing,
}

fn argument(arg: Arg<'_>) -> VARIANT {
    let mut value = VARIANT::default();
    let inner = unsafe { &mut value.Anonymous.Anonymous };
    match arg {
        Arg::Int(v) => {
            inner.vt = VT_I4;
            inner.Anonymous.lVal = v;
        }
        Arg::Float(v) => {
            inner.vt = VT_R8;
            inner.Anonymous.dblVal = v;
        }
        Arg::Bool(v) => {
            inner.vt = VT_BOOL;
            inner.Anonymous.boolVal.0 = if v { -1 } else { 0 };
        }
        Arg::Text(v) => {
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(v));
        }
        Arg::Missing => {
            inner.vt = VT_ERROR;
            inner.Anonymous.scode = 0x8002_0004_u32 as i32;
        }
    }
    value
}

struct Value(VARIANT);
impl Drop for Value {
    fn drop(&mut self) {
        unsafe {
            let _ = VariantClear(&mut self.0);
        }
    }
}
impl Value {
    fn inner(&self) -> &windows::Win32::System::Variant::VARIANT_0_0 {
        unsafe { &self.0.Anonymous.Anonymous }
    }
    fn dispatch(&self) -> Result<IDispatch> {
        let inner = self.inner();
        ensure!(inner.vt == VT_DISPATCH, "Excel returned a non-object value");
        unsafe { (*inner.Anonymous.pdispVal).clone() }.context("Excel returned a null object")
    }
    fn int(&self) -> Result<i32> {
        let inner = self.inner();
        ensure!(inner.vt == VT_I4, "Excel returned a non-integer value");
        Ok(unsafe { inner.Anonymous.lVal })
    }
    fn float(&self) -> Result<f64> {
        let inner = self.inner();
        match inner.vt {
            VT_R8 => Ok(unsafe { inner.Anonymous.dblVal }),
            VT_I4 => Ok(unsafe { inner.Anonymous.lVal } as f64),
            _ => bail!("Excel returned a non-numeric value"),
        }
    }
    fn string(&self) -> Result<String> {
        let inner = self.inner();
        ensure!(inner.vt == VT_BSTR, "Excel returned a non-string value");
        Ok(unsafe { (*inner.Anonymous.bstrVal).to_string() })
    }
    fn boolean(&self) -> Result<bool> {
        let inner = self.inner();
        ensure!(inner.vt == VT_BOOL, "Excel returned a non-boolean value");
        Ok(unsafe { inner.Anonymous.boolVal.0 != 0 })
    }
}

fn invoke(
    object: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
    args: Vec<Arg<'_>>,
) -> Result<Value> {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let name = PCWSTR(wide.as_ptr());
    let mut id = 0;
    unsafe { object.GetIDsOfNames(&GUID::zeroed(), &name, 1, 0, &mut id) }
        .with_context(|| format!("Excel member {name:?} was not found"))?;
    let mut values: Vec<VARIANT> = args.into_iter().rev().map(argument).collect();
    let mut property_id = -3;
    let mut params = DISPPARAMS {
        rgvarg: values.as_mut_ptr(),
        rgdispidNamedArgs: std::ptr::null_mut(),
        cArgs: values.len() as u32,
        cNamedArgs: 0,
    };
    if flags == DISPATCH_PROPERTYPUT {
        params.rgdispidNamedArgs = &mut property_id;
        params.cNamedArgs = 1;
    }
    let mut result = VARIANT::default();
    let status = unsafe {
        object.Invoke(
            id,
            &GUID::zeroed(),
            0,
            flags,
            &params,
            Some(&mut result),
            None,
            None,
        )
    };
    for value in &mut values {
        unsafe {
            let _ = VariantClear(value);
        }
    }
    let result = Value(result);
    status.with_context(|| format!("Excel COM call {name:?} failed"))?;
    Ok(result)
}

fn get(object: &IDispatch, name: &str) -> Result<Value> {
    invoke(object, name, DISPATCH_PROPERTYGET, vec![])
}
fn prop(object: &IDispatch, name: &str) -> Result<IDispatch> {
    get(object, name)?.dispatch()
}
fn put(object: &IDispatch, name: &str, arg: Arg<'_>) -> Result<()> {
    invoke(object, name, DISPATCH_PROPERTYPUT, vec![arg])?;
    Ok(())
}
fn call(object: &IDispatch, name: &str, args: Vec<Arg<'_>>) -> Result<Value> {
    invoke(object, name, DISPATCH_METHOD, args)
}
fn method(object: &IDispatch, name: &str, args: Vec<Arg<'_>>) -> Result<IDispatch> {
    call(object, name, args)?.dispatch()
}
fn item(object: &IDispatch, arg: Arg<'_>) -> Result<IDispatch> {
    invoke(object, "Item", DISPATCH_PROPERTYGET, vec![arg])?.dispatch()
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
    let existing = excel_processes()?;
    let clsid = unsafe { CLSIDFromProgID(w!("Excel.Application"))? };
    let excel: IDispatch = unsafe { CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)? };
    let hwnd = HWND(get(&excel, "Hwnd")?.int()? as isize as *mut _);
    let mut excel_id = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut excel_id));
    }
    ensure!(
        excel_id != 0 && !existing.contains(&excel_id),
        "Excel did not create an isolated process; existing user sessions are not used."
    );
    // Only this newly created Excel process may be terminated if isolation fails.
    let job = match own_excel(excel_id) {
        Ok(job) => job,
        Err(error) => {
            if let Ok(process) = unsafe { OpenProcess(PROCESS_TERMINATE, false, excel_id) } {
                let process = Handle(process);
                unsafe {
                    let _ = TerminateProcess(process.0, 1);
                }
            }
            return Err(error);
        }
    };
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
