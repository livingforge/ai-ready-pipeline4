//! Excel's late-bound COM automation lives in a disposable STA process.
#![cfg(windows)]

use crate::office_com::{
    Apartment, Arg, Handle, call, get, invoke, isolated_application, item, method, prop, put,
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::json;
use std::{fs, path::Path, time::Duration};
use windows::{
    Win32::{
        Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0},
        System::{
            Com::{DISPATCH_PROPERTYGET, IDispatch},
            Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        },
    },
    core::{HRESULT, w},
};

#[derive(Deserialize)]
struct Request {
    source: String,
    sheet: String,
    range: Option<String>,
    output: String,
    result: String,
    /// Where a failed render describes what failed, for the parent to report.
    failure: String,
    /// How long to wait for another render to finish with the clipboard.
    lock_wait_seconds: u64,
    /// Whether the formulas are calculated before the range is drawn, as
    /// Excel does opening a workbook whose results ARP left out; otherwise
    /// the results saved in the workbook are drawn.
    #[serde(default)]
    recalculate: bool,
}

struct RenderMutex(Handle);
impl RenderMutex {
    /// Takes the clipboard from other renders of this session, waiting up to
    /// `seconds` for one to finish. A render whose worker was killed leaves the
    /// mutex abandoned, which is taken as free.
    fn lock(seconds: u64) -> Result<Self> {
        let handle = Handle(unsafe { CreateMutexW(None, false, w!("Local\\ARP4ExcelRender"))? });
        let millis = u32::try_from(seconds.saturating_mul(1000)).unwrap_or(u32::MAX - 1);
        let state = unsafe { WaitForSingleObject(handle.0, millis) };
        ensure!(
            state == WAIT_OBJECT_0 || state == WAIT_ABANDONED,
            "Another Excel render used the clipboard for {seconds} seconds; retry after it finishes."
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

/// The step of a render that failed, kept in the error so that the parent can
/// say what failed and how to recover: source, clipboard_lock, launch, open,
/// sheet, range, calibrate, budget, prepare, copy_picture, paste, export or image.
#[derive(Debug, Clone, Copy)]
struct Stage(&'static str);
impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Excel render step {}", self.0)
    }
}

/// How many times a clipboard call was tried before it failed.
#[derive(Debug, Clone, Copy)]
struct Attempts(u32);
impl std::fmt::Display for Attempts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tried {} times", self.0)
    }
}

/// A range larger than one image may be, with the cell ranges that each fit:
/// rendered one by one, they show all of it at full size.
#[derive(Debug)]
struct OverBudget {
    range: String,
    width: f64,
    height: f64,
    plan: Tiles,
}

/// How a range over the budget splits into images.
#[derive(Debug, PartialEq)]
enum Tiles {
    /// The cell ranges, row by row, that each render within the budget.
    Fit(Vec<String>),
    /// About this many images: more than [`MAX_TILES`], so none are listed.
    TooMany(usize),
    /// A single row or column is larger than one image may be.
    TooLarge,
}

/// The most tiles a range over the budget is split into; a larger range is
/// narrowed first, to the cells a drawing actually covers.
const MAX_TILES: usize = 100;
impl std::fmt::Display for OverBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Range {} exceeds the render budget ({:.0}x{:.0} px; at most {MAX_SIDE} px a side and {MAX_PIXELS} pixels); ",
            self.range, self.width, self.height
        )?;
        match &self.plan {
            Tiles::Fit(tiles) => write!(f, "render it as {} tiles", tiles.len()),
            Tiles::TooMany(count) => write!(
                f,
                "it would take about {count} images; give a narrower range"
            ),
            Tiles::TooLarge => write!(f, "a single row or column is larger than the budget"),
        }
    }
}
impl std::error::Error for OverBudget {}

/// The most pixels a rendered image may have on a side and in all.
const MAX_SIDE: f64 = 8192.0;
const MAX_PIXELS: f64 = 32_000_000.0;
/// A tile is planned this much under the budget, for Excel's rounding.
const TILE_MARGIN: f64 = 0.95;
/// The side, in points, of the empty chart exported to measure pixels per point.
const PROBE_POINTS: f64 = 144.0;

/// Whether a range of `width` x `height` points renders within the budget at
/// `ratio` pixels per point.
fn within_budget(width: f64, height: f64, ratio: f64) -> bool {
    let (w, h) = (width * ratio, height * ratio);
    w <= MAX_SIDE && h <= MAX_SIDE && w * h <= MAX_PIXELS
}

/// The pixels Excel's PNG export gives a point on this machine: 4/3 at 100 %
/// display scaling and more above it. An empty chart is exported and measured,
/// so that the budget and the tiles hold for the image Excel actually writes.
fn pixels_per_point(scratch: &IDispatch, probe: &Path) -> Result<f64> {
    let scratch_sheet = item(&prop(scratch, "Worksheets")?, Arg::Int(1))?;
    let chart_object = method(
        &method(&scratch_sheet, "ChartObjects", vec![])?,
        "Add",
        vec![
            Arg::Int(0),
            Arg::Int(0),
            Arg::Float(PROBE_POINTS),
            Arg::Float(PROBE_POINTS),
        ],
    )?;
    let exported = call(
        &prop(&chart_object, "Chart")?,
        "Export",
        vec![
            Arg::Text(&probe.to_string_lossy()),
            Arg::Text("PNG"),
            Arg::Bool(false),
        ],
    )?
    .boolean()?;
    let _ = call(&chart_object, "Delete", vec![]);
    ensure!(exported, "Excel did not export the measuring chart.");
    let bytes = fs::read(probe)?;
    let _ = fs::remove_file(probe);
    ensure!(
        bytes.len() >= 24 && &bytes[12..16] == b"IHDR",
        "Invalid measuring image"
    );
    let ratio = f64::from(u32::from_be_bytes(bytes[16..20].try_into()?)) / PROBE_POINTS;
    ensure!(
        (0.5..=8.0).contains(&ratio),
        "Unexpected export scale {ratio:.2} px/pt"
    );
    Ok(ratio)
}

/// Splits `first..=last` into consecutive groups whose extent is at most
/// `limit`, each as large as fits: `extent(from, to)` measures a group, and
/// growing a group never shrinks it, so the end of each is found by halving.
/// None when a single row or column is larger than `limit`.
fn spans(
    first: i32,
    last: i32,
    limit: f64,
    extent: &mut impl FnMut(i32, i32) -> Result<f64>,
) -> Result<Option<Vec<(i32, i32)>>> {
    let mut groups = Vec::new();
    let mut start = first;
    while start <= last {
        if extent(start, start)? > limit {
            return Ok(None);
        }
        let (mut low, mut high) = (start, last);
        while low < high {
            let middle = low + (high - low + 1) / 2;
            if extent(start, middle)? <= limit {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        groups.push((start, low));
        start = low + 1;
    }
    Ok(Some(groups))
}

/// How the range splits into images that each render within the budget.
/// `width(c1, c2)` and `height(r1, r2)` are the points a span of columns or
/// rows takes. A range that would need more than [`MAX_TILES`] images is only
/// counted, from its whole height, before its rows are planned.
fn plan_tiles(
    (column, row, last_column, last_row): (i32, i32, i32, i32),
    ratio: f64,
    mut width: impl FnMut(i32, i32) -> Result<f64>,
    mut height: impl FnMut(i32, i32) -> Result<f64>,
) -> Result<Tiles> {
    let side = MAX_SIDE / ratio * TILE_MARGIN;
    let total = width(column, last_column)?;
    if (total / side).ceil() as usize > MAX_TILES {
        return Ok(Tiles::TooMany((total / side).ceil() as usize));
    }
    let Some(columns) = spans(column, last_column, side, &mut width)? else {
        return Ok(Tiles::TooLarge);
    };
    let mut widest: f64 = 1.0;
    for &(from, to) in &columns {
        widest = widest.max(width(from, to)?);
    }
    // The widest tile leaves the least height under the pixel budget.
    let area = MAX_PIXELS / (ratio * ratio);
    let tall = side.min(area / widest * TILE_MARGIN);
    let estimate = columns.len() * (height(row, last_row)? / tall).ceil() as usize;
    if estimate > MAX_TILES {
        return Ok(Tiles::TooMany(estimate));
    }
    let Some(rows) = spans(row, last_row, tall, &mut height)? else {
        return Ok(Tiles::TooLarge);
    };
    let mut tiles = Vec::new();
    for &(top, bottom) in &rows {
        for &(left, right) in &columns {
            let (first, last) = (address(left, top), address(right, bottom));
            tiles.push(if first == last {
                first
            } else {
                format!("{first}:{last}")
            });
        }
    }
    Ok(Tiles::Fit(tiles))
}

/// What a failed render reports to the parent: the step, the COM error code,
/// the clipboard tries and, for a range over the budget, the tiles.
fn describe(error: &anyhow::Error, request: &Request) -> serde_json::Value {
    let mut failure = json!({
        "stage": error.downcast_ref::<Stage>().map(|stage| stage.0),
        "message": format!("{error:#}"),
        "sheet": request.sheet,
        "requested_range": request.range,
    });
    if let Some(com) = error.downcast_ref::<windows::core::Error>() {
        failure["hresult"] = json!(format!("0x{:08X}", com.code().0 as u32));
    }
    if let Some(attempts) = error.downcast_ref::<Attempts>() {
        failure["attempts"] = json!(attempts.0);
    }
    if let Some(over) = error.downcast_ref::<OverBudget>() {
        failure["range"] = json!(over.range);
        failure["width"] = json!(over.width.round());
        failure["height"] = json!(over.height.round());
        let (tiles, needed) = match &over.plan {
            Tiles::Fit(tiles) => (tiles.clone(), tiles.len()),
            Tiles::TooMany(count) => (Vec::new(), *count),
            Tiles::TooLarge => (Vec::new(), 0),
        };
        failure["tiles"] = json!(tiles);
        failure["tiles_needed"] = json!(needed);
    }
    failure
}

/// The error Excel gives for a failed call, which is also what a clipboard
/// call gives while another program has the clipboard open.
const EXCEL_CALL_FAILED: HRESULT = HRESULT(0x800A_03EC_u32 as i32);

/// How many times a clipboard call is tried, waiting longer after each try.
const CLIPBOARD_ATTEMPTS: u32 = 5;

/// Runs a clipboard call of Excel, trying it again while it fails as it does
/// when another program has the clipboard open: clipboard viewers, history and
/// sync read it right after it changes. A call failing that way for another
/// reason fails after the last try.
fn on_clipboard<T>(mut call: impl FnMut() -> Result<T>) -> Result<T> {
    let mut attempt = 1;
    loop {
        match call() {
            Err(error) if attempt < CLIPBOARD_ATTEMPTS && excel_call_failed(&error) => {
                std::thread::sleep(Duration::from_millis(100 * u64::from(attempt)));
                attempt += 1;
            }
            Err(error) => return Err(error.context(Attempts(attempt))),
            outcome => return outcome,
        }
    }
}

fn excel_call_failed(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<windows::core::Error>()
        .is_some_and(|error| error.code() == EXCEL_CALL_FAILED)
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
    let outcome = render_request(&request);
    if let Err(error) = &outcome {
        fs::write(
            &request.failure,
            serde_json::to_vec(&describe(error, &request))?,
        )?;
    }
    outcome
}

fn render_request(request: &Request) -> Result<()> {
    super::render::validate_source(Path::new(&request.source)).context(Stage("source"))?;
    let _mutex = RenderMutex::lock(request.lock_wait_seconds).context(Stage("clipboard_lock"))?;
    let _apartment = Apartment::new().context(Stage("launch"))?;
    let (excel, job) =
        isolated_application("Excel.Application", "excel.exe").context(Stage("launch"))?;
    let outcome = render_with_excel(&excel, request);
    // Drop the job after a best-effort graceful close; a stuck COM call is handled by
    // the parent killing this worker, which closes the job handle and its Excel.
    let _ = call(&excel, "Quit", vec![]);
    drop(job);
    outcome
}

fn render_with_excel(excel: &IDispatch, request: &Request) -> Result<()> {
    let (scratch, source) = open_quietly(excel, &request.source).context(Stage("open"))?;
    if request.recalculate {
        call(excel, "CalculateFull", vec![]).context(Stage("open"))?;
    }
    let outcome = render_range(excel, &scratch, &source, request);
    let _ = call(&source, "Close", vec![Arg::Bool(false)]);
    let _ = call(&scratch, "Close", vec![Arg::Bool(false)]);
    outcome
}

/// Opens the workbook at `path` read-only in `excel` with alerts, events,
/// macros, link updates and calculation off, beside a new scratch workbook
/// that holds the calculation setting: (scratch, workbook).
fn open_quietly(excel: &IDispatch, path: &str) -> Result<(IDispatch, IDispatch)> {
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
            Arg::Text(path),
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
    Ok((scratch, source))
}

#[derive(Deserialize)]
struct CalculationRequest {
    source: String,
    output: String,
    result: String,
}

/// Calculates every formula of a workbook as Excel does opening it, and
/// saves the calculated copy for its results to be read.
pub fn calculation_worker(request_path: &Path) -> Result<()> {
    let request: CalculationRequest = serde_json::from_slice(&fs::read(request_path)?)?;
    super::render::validate_source(Path::new(&request.source))?;
    let _apartment = Apartment::new()?;
    let (excel, job) = isolated_application("Excel.Application", "excel.exe")?;
    let outcome = calculate_with_excel(&excel, &request);
    let _ = call(&excel, "Quit", vec![]);
    drop(job);
    outcome
}

fn calculate_with_excel(excel: &IDispatch, request: &CalculationRequest) -> Result<()> {
    let (scratch, source) = open_quietly(excel, &request.source)?;
    let saved = call(excel, "CalculateFull", vec![])
        .and_then(|_| call(&source, "SaveCopyAs", vec![Arg::Text(&request.output)]));
    let _ = call(&source, "Close", vec![Arg::Bool(false)]);
    let _ = call(&scratch, "Close", vec![Arg::Bool(false)]);
    saved?;
    let version = get(excel, "Version")?.string()?;
    fs::write(
        &request.result,
        serde_json::to_vec(&json!({"engine":"excel-com","version":version}))?,
    )?;
    Ok(())
}

fn render_range(
    excel: &IDispatch,
    scratch: &IDispatch,
    source: &IDispatch,
    request: &Request,
) -> Result<()> {
    let sheet = (|| -> Result<IDispatch> {
        let sheet = item(&prop(source, "Worksheets")?, Arg::Text(&request.sheet))?;
        ensure!(
            get(&sheet, "Visible")?.int()? == -1,
            "The requested sheet is hidden. Render a visible sheet; hidden state is not changed."
        );
        call(&sheet, "Activate", vec![])?;
        Ok(sheet)
    })()
    .context(Stage("sheet"))?;
    let cells = match &request.range {
        Some(range) if !range.is_empty() => invoke(
            &sheet,
            "Range",
            DISPATCH_PROPERTYGET,
            vec![Arg::Text(range)],
        )
        .and_then(|cells| cells.dispatch())
        .context(Stage("range"))?,
        _ => prop(&sheet, "UsedRange").context(Stage("range"))?,
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
    if width <= 0.0 || height <= 0.0 {
        return Err(anyhow::anyhow!("The requested range has no visible area."))
            .context(Stage("range"));
    }
    let probe = Path::new(&request.output).with_extension("probe.png");
    let ratio = pixels_per_point(scratch, &probe).context(Stage("calibrate"))?;
    if !within_budget(width, height, ratio) {
        // Measure spans of whole columns and rows on the sheet to plan the tiles.
        let span = |from: String, to: String, what: &str| -> Result<f64> {
            let cells = invoke(
                &sheet,
                "Range",
                DISPATCH_PROPERTYGET,
                vec![Arg::Text(&format!("{from}:{to}"))],
            )?
            .dispatch()?;
            get(&cells, what)?.float()
        };
        let tiles = plan_tiles(
            (column, row, last_column, last_row),
            ratio,
            |from, to| span(address(from, row), address(to, row), "Width"),
            |from, to| span(address(column, from), address(column, to), "Height"),
        )
        .context(Stage("budget"))?;
        let over = OverBudget {
            range,
            width: width * ratio,
            height: height * ratio,
            plan: tiles,
        };
        return Err(anyhow::Error::new(over).context(Stage("budget")));
    }
    render_picture(
        excel, scratch, &sheet, &cells, request, &range, width, height, ratio,
    )
}

/// Copies `cells` as a picture into a chart of the scratch workbook and exports
/// it as PNG, recording what was drawn.
#[allow(clippy::too_many_arguments)]
fn render_picture(
    excel: &IDispatch,
    scratch: &IDispatch,
    sheet: &IDispatch,
    cells: &IDispatch,
    request: &Request,
    range: &str,
    width: f64,
    height: f64,
    ratio: f64,
) -> Result<()> {
    let prepared = (|| -> Result<(IDispatch, IDispatch)> {
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
        call(sheet, "Activate", vec![])?;
        Ok((chart_object, chart))
    })()
    .context(Stage("prepare"))?;
    let (chart_object, chart) = prepared;
    on_clipboard(|| call(cells, "CopyPicture", vec![Arg::Int(1), Arg::Int(-4147)]))
        .context(Stage("copy_picture"))?;
    call(scratch, "Activate", vec![])
        .and_then(|_| call(&chart_object, "Activate", vec![]))
        .context(Stage("paste"))?;
    on_clipboard(|| call(&chart, "Paste", vec![])).context(Stage("paste"))?;
    let exported = call(
        &chart,
        "Export",
        vec![
            Arg::Text(&request.output),
            Arg::Text("PNG"),
            Arg::Bool(false),
        ],
    )
    .and_then(|done| done.boolean())
    .context(Stage("export"))?;
    if !exported {
        return Err(anyhow::anyhow!("Excel PNG export failed.")).context(Stage("export"));
    }
    let (width, height) = png_dimensions(Path::new(&request.output)).context(Stage("image"))?;
    let version = get(excel, "Version")?.string()?;
    fs::write(
        &request.result,
        serde_json::to_vec(&json!({
            "engine":"excel-com", "version":version, "sheet":request.sheet,
            "range":range, "width":width, "height":height, "clipboard_changed":true,
            "pixels_per_point":(ratio * 1000.0).round() / 1000.0,
        }))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    /// An error as a failed COM call returns it.
    fn failed(code: HRESULT) -> anyhow::Error {
        Err::<(), _>(windows::core::Error::from(code))
            .context("Office COM call \"CopyPicture\" failed")
            .unwrap_err()
    }

    #[test]
    fn a_render_waits_for_another_to_finish_with_the_clipboard() {
        let (held, holding) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _lock = RenderMutex::lock(0).unwrap();
            held.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(500));
        });
        holding.recv().unwrap();
        let busy = std::thread::spawn(|| RenderMutex::lock(0).map(|_| ()))
            .join()
            .unwrap()
            .unwrap_err();
        assert!(busy.to_string().contains("Another Excel render"), "{busy}");
        let waited = std::thread::spawn(|| {
            let started = std::time::Instant::now();
            RenderMutex::lock(10).map(|_| started.elapsed())
        })
        .join()
        .unwrap()
        .unwrap();
        assert!(waited >= Duration::from_millis(200), "{waited:?}");
        holder.join().unwrap();
    }

    #[test]
    fn a_range_over_the_budget_is_planned_as_tiles_that_each_fit() {
        // 40 columns of 300 points and 500 rows of 20 points: 16000 x 13333 px at 100 %.
        let width = |from: i32, to: i32| Ok(f64::from(to - from + 1) * 300.0);
        let height = |from: i32, to: i32| Ok(f64::from(to - from + 1) * 20.0);
        let parse = |cell: &str| -> (i32, i32) {
            let split = cell.find(|c: char| c.is_ascii_digit()).unwrap();
            let column = cell[..split]
                .bytes()
                .fold(0, |n, b| n * 26 + i32::from(b - b'A' + 1));
            (column, cell[split..].parse().unwrap())
        };
        // At 100 % display scaling and at 150 %, where Excel exports 2 px a point.
        for ratio in [4.0 / 3.0, 2.0] {
            assert!(!within_budget(
                width(1, 40).unwrap(),
                height(1, 500).unwrap(),
                ratio
            ));
            let Tiles::Fit(tiles) = plan_tiles((1, 1, 40, 500), ratio, width, height).unwrap()
            else {
                panic!("the range fits in tiles")
            };
            let mut cells = 0;
            for tile in &tiles {
                let (first, last) = tile.split_once(':').unwrap();
                let ((c1, r1), (c2, r2)) = (parse(first), parse(last));
                assert!(
                    within_budget(width(c1, c2).unwrap(), height(r1, r2).unwrap(), ratio),
                    "{tile} at {ratio}"
                );
                cells += (c2 - c1 + 1) * (r2 - r1 + 1);
            }
            assert_eq!(cells, 40 * 500, "the tiles cover the range once");
            if ratio < 1.5 {
                // Columns of at most 5836 points: 19 columns (5700); rows of at most
                // 18M / 5700 x 0.95 = 3000 points: 150.
                assert_eq!(tiles[0], "A1:S150");
                assert_eq!(tiles[1], "T1:AL150");
                assert_eq!(tiles[2], "AM1:AN150");
                assert_eq!(tiles.last().unwrap(), "AM451:AN500");
            } else {
                // Columns of at most 3891 points: 12 columns (3600).
                assert_eq!(
                    tiles[0]
                        .split_once(':')
                        .unwrap()
                        .1
                        .trim_end_matches(char::is_numeric),
                    "L"
                );
            }
        }
        let ratio = 4.0 / 3.0;
        // A single column wider than an image cannot be tiled.
        let wide = |from: i32, to: i32| Ok(f64::from(to - from + 1) * 7000.0);
        assert_eq!(
            plan_tiles((1, 1, 3, 10), ratio, wide, height).unwrap(),
            Tiles::TooLarge
        );
        // A whole sheet is counted from its size, never planned row by row.
        let mut probes = 0;
        let counted = plan_tiles((1, 1, 16384, 1_048_576), ratio, width, |from, to| {
            probes += 1;
            height(from, to)
        })
        .unwrap();
        assert!(
            matches!(counted, Tiles::TooMany(n) if n > MAX_TILES),
            "{counted:?}"
        );
        assert!(probes <= 1, "{probes}");
    }

    #[test]
    fn a_failure_reports_its_step_com_code_and_tries() {
        let request = Request {
            source: String::new(),
            sheet: "S".into(),
            range: Some("A1:B2".into()),
            output: String::new(),
            result: String::new(),
            failure: String::new(),
            lock_wait_seconds: 0,
            recalculate: false,
        };
        let error = on_clipboard(|| -> Result<()> { Err(failed(EXCEL_CALL_FAILED)) })
            .context(Stage("copy_picture"))
            .unwrap_err();
        let failure = describe(&error, &request);
        assert_eq!(failure["stage"], "copy_picture");
        assert_eq!(failure["hresult"], "0x800A03EC");
        assert_eq!(failure["attempts"], CLIPBOARD_ATTEMPTS);
        assert_eq!(failure["sheet"], "S");
        let over = OverBudget {
            range: "A1:Z40".into(),
            width: 9000.4,
            height: 100.0,
            plan: Tiles::Fit(vec!["A1:M40".into(), "N1:Z40".into()]),
        };
        let failure = describe(&anyhow::Error::new(over).context(Stage("budget")), &request);
        assert_eq!(
            (failure["stage"].clone(), failure["width"].clone()),
            (json!("budget"), json!(9000.0))
        );
        assert_eq!(failure["tiles"], json!(["A1:M40", "N1:Z40"]));
        assert_eq!(failure["tiles_needed"], 2);
        let over = OverBudget {
            range: "A1:XFD1048576".into(),
            width: 1e6,
            height: 1e7,
            plan: Tiles::TooMany(5000),
        };
        let failure = describe(&anyhow::Error::new(over).context(Stage("budget")), &request);
        assert_eq!(failure["tiles"], json!([]));
        assert_eq!(failure["tiles_needed"], 5000);
    }

    #[test]
    fn clipboard_calls_are_tried_again_while_excel_fails_them() {
        let mut tries = 0;
        let result = on_clipboard(|| {
            tries += 1;
            if tries < 3 {
                Err(failed(EXCEL_CALL_FAILED))
            } else {
                Ok(tries)
            }
        });
        assert_eq!(result.unwrap(), 3);

        let mut tries = 0;
        let result = on_clipboard(|| -> Result<()> {
            tries += 1;
            Err(failed(EXCEL_CALL_FAILED))
        });
        assert!(result.is_err());
        assert_eq!(tries, CLIPBOARD_ATTEMPTS);

        // Other failures are not the clipboard's: they fail at once.
        let mut tries = 0;
        let result = on_clipboard(|| -> Result<()> {
            tries += 1;
            Err(failed(HRESULT(0x8002_0009_u32 as i32)))
        });
        assert!(result.is_err());
        assert_eq!(tries, 1);
    }
}
