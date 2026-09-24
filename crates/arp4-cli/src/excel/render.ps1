param([Parameter(Mandatory=$true)][string]$Request)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$requestData = Get-Content -LiteralPath $Request -Raw -Encoding UTF8 | ConvertFrom-Json
$encoding = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $encoding

# The job owns only the newly created Excel process. Killing this helper closes
# the job handle, so a timed-out COM call cannot keep an owned Excel alive.
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
public sealed class ArpExcelJob : IDisposable {
    [StructLayout(LayoutKind.Sequential)] struct Basic {
        public long processTime, jobTime; public uint flags;
        public UIntPtr minWorking, maxWorking; public uint active;
        public UIntPtr affinity; public uint priority, scheduling;
    }
    [StructLayout(LayoutKind.Sequential)] struct Io {
        public ulong readOps, writeOps, otherOps, readBytes, writeBytes, otherBytes;
    }
    [StructLayout(LayoutKind.Sequential)] struct Extended {
        public Basic basic; public Io io;
        public UIntPtr processMemory, jobMemory, peakProcess, peakJob;
    }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr attr, string name);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job, int type, ref Extended data, uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    private IntPtr job;
    public static int ProcessId(int window) { uint id; GetWindowThreadProcessId(new IntPtr(window), out id); return (int)id; }
    public ArpExcelJob(int id) {
        job = CreateJobObject(IntPtr.Zero, null);
        Extended limits = new Extended(); limits.basic.flags = 0x2000;
        using (Process p = Process.GetProcessById(id)) {
            if (job == IntPtr.Zero || !SetInformationJobObject(job, 9, ref limits, (uint)Marshal.SizeOf(typeof(Extended))) || !AssignProcessToJobObject(job, p.Handle)) {
                int error = Marshal.GetLastWin32Error(); Dispose();
                throw new System.ComponentModel.Win32Exception(error, "Cannot isolate Excel renderer");
            }
        }
    }
    public void Dispose() { if (job != IntPtr.Zero) { CloseHandle(job); job = IntPtr.Zero; } }
}
'@

$excel = $null
$workbook = $null
$scratch = $null
$job = $null
$owned = $false
$mutex = New-Object System.Threading.Mutex($false, 'Local\ARP4ExcelRender')
$locked = $false
function CellAddress([int]$column, [int]$row) {
    $letters = ''
    while ($column -gt 0) {
        $letters = [char](65 + (($column - 1) % 26)) + $letters
        $column = [math]::Floor(($column - 1) / 26)
    }
    return "$letters$row"
}
try {
    try { $locked = $mutex.WaitOne(0) } catch [System.Threading.AbandonedMutexException] { $locked = $true }
    if (-not $locked) { throw 'Another Excel render is using the clipboard; retry after it finishes.' }
    $existing = @(Get-Process -Name EXCEL -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
    $excel = New-Object -ComObject Excel.Application
    $excelId = [ArpExcelJob]::ProcessId($excel.Hwnd)
    if ($excelId -le 0 -or $existing -contains $excelId) { throw 'Excel did not create an isolated process; existing user sessions are not used.' }
    $owned = $true
    $job = New-Object ArpExcelJob($excelId)
    $excel.Visible = $false
    $excel.DisplayAlerts = $false
    $excel.EnableEvents = $false
    $excel.AskToUpdateLinks = $false
    $excel.CopyObjectsWithCells = $true
    $excel.AutomationSecurity = 3
    # A blank workbook lets manual calculation be set before opening the source.
    $scratch = $excel.Workbooks.Add()
    $excel.Calculation = -4135
    $missing = [Type]::Missing
    $workbook = $excel.Workbooks.Open($requestData.source, 0, $true, $missing, '', '', $true, $missing, $missing, $false, $false, $missing, $false)
    $sheet = $workbook.Worksheets.Item([string]$requestData.sheet)
    if ($sheet.Visible -ne -1) { throw 'The requested sheet is hidden. Render a visible sheet; hidden state is not changed.' }
    $sheet.Activate()
    # Do not emit a COM Range through the PowerShell pipeline: it enumerates cells.
    if ($requestData.range) { $cells = $sheet.Range([string]$requestData.range) } else { $cells = $sheet.UsedRange }
    $first = CellAddress ([int]$cells.Column) ([int]$cells.Row)
    $last = CellAddress ([int]$cells.Column + [int]$cells.Columns.Count - 1) ([int]$cells.Row + [int]$cells.Rows.Count - 1)
    $address = if ($first -eq $last) { $first } else { "${first}:$last" }
    $width = [double]$cells.Width
    $height = [double]$cells.Height
    if ($width -le 0 -or $height -le 0) { throw 'The requested range has no visible area.' }
    # Excel dimensions are points; budget at 96dpi. Never silently shrink a complex table.
    if ($width * 4 / 3 -gt 8192 -or $height * 4 / 3 -gt 8192 -or $width * $height * 16 / 9 -gt 32000000) {
        throw 'Range exceeds the render budget; request smaller explicit cell ranges.'
    }
    $chartObject = $scratch.Worksheets.Item(1).ChartObjects().Add(0, 0, $width, $height)
    $chart = $chartObject.Chart
    $chart.ChartArea.Format.Line.Visible = 0
    $sheet.Activate()
    $cells.CopyPicture(1, -4147)
    $scratch.Activate()
    $chartObject.Activate()
    $chart.Paste()
    if (-not $chart.Export([string]$requestData.output, 'PNG', $false)) { throw 'Excel PNG export failed.' }
    Add-Type -AssemblyName System.Drawing
    $bitmap = [System.Drawing.Bitmap]::FromFile([string]$requestData.output)
    try {
        if ($bitmap.Width -le 1 -or $bitmap.Height -le 1 -or $bitmap.Width -gt 8192 -or $bitmap.Height -gt 8192 -or [long]$bitmap.Width * $bitmap.Height -gt 32000000) { throw 'Invalid or oversized rendered image.' }
        $result = @{engine='excel-com';version=[string]$excel.Version;sheet=[string]$requestData.sheet;range=[string]$address;width=$bitmap.Width;height=$bitmap.Height;clipboard_changed=$true}
        [IO.File]::WriteAllText([string]$requestData.result, ($result | ConvertTo-Json -Compress), $encoding)
    } finally { $bitmap.Dispose() }
} catch {
    throw ('Excel render at script line {0}: {1}' -f $_.InvocationInfo.ScriptLineNumber, $_.Exception.Message)
} finally {
    if ($owned) {
        if ($workbook) { try { $workbook.Close($false) } catch {} }
        if ($scratch) { try { $scratch.Close($false) } catch {} }
        if ($excel) { try { $excel.Quit() } catch {} }
    }
    if ($job) { $job.Dispose() }
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
