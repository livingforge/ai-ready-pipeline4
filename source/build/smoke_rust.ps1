param([Parameter(Mandatory = $true)][string]$Zip)
$ErrorActionPreference = 'Stop'
$archive = (Resolve-Path -LiteralPath $Zip).Path
$expected = (Get-Content -LiteralPath "$archive.sha256" -Raw).Split(' ')[0]
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'ZIP checksum mismatch' }
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) "arp-rust-smoke-$([guid]::NewGuid().ToString('N'))"
$unpacked = Join-Path $testRoot 'unpacked'
New-Item -ItemType Directory -Path $unpacked | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::ExtractToDirectory($archive, $unpacked)
$binary = Join-Path $unpacked 'arp4.exe'
$project = Join-Path $testRoot 'project with spaces'
New-Item -ItemType Directory -Path $project | Out-Null
$savedPath = $env:PATH
try {
    $env:PATH = ''
    $report = & $binary doctor --format json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $report.implementation -ne 'rust' -or $report.release_ready -ne $false) { throw 'Invalid preview capabilities' }
    & $binary skills install --root $project --agent github
    if ($LASTEXITCODE -ne 0) { throw 'Skill installation failed' }
    $skill = Join-Path $project '.github/skills/arp4/SKILL.md'
    if (-not (Test-Path -LiteralPath $skill)) { throw 'Skill missing' }
    if (Test-Path -LiteralPath (Join-Path $project '.claude')) { throw 'Wrong agent installed' }
    $schema = & $binary documents schema mappings | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $schema.properties.schema_version.const -ne '2') { throw 'Schema contract mismatch' }
    [System.IO.File]::WriteAllText($skill, 'user customization')
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $binary
    $info.Arguments = "skills install --root `"$project`" --agent github"
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $process = [System.Diagnostics.Process]::Start($info)
    $process.WaitForExit()
    if ($process.ExitCode -ne 2) { throw 'Local skill change was not rejected' }
    if ([System.IO.File]::ReadAllText($skill) -ne 'user customization') { throw 'Local skill change was overwritten' }
    # Build an independent minimal workbook using .NET, then run the full native workflow.
    $source = Join-Path $project 'source.xlsx'
    $parts = [ordered]@{
        '[Content_Types].xml' = '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>'
        '_rels/.rels' = '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>'
        'xl/workbook.xml' = '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="r1"/></sheets></workbook>'
        'xl/_rels/workbook.xml.rels' = '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>'
        'xl/worksheets/sheet1.xml' = '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Value</t></is></c><c r="B1" t="n"><v>10</v></c></row></sheetData></worksheet>'
    }
    $book = [System.IO.Compression.ZipFile]::Open($source, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($part in $parts.Keys) {
            $writer = New-Object System.IO.StreamWriter($book.CreateEntry($part).Open())
            try { $writer.Write($parts[$part]) } finally { $writer.Dispose() }
        }
    } finally { $book.Dispose() }
    function Invoke-ArpJson([string[]]$Command) {
        $text = & $binary documents @Command --root $project
        if ($LASTEXITCODE -ne 0) { throw "Native command failed: $Command" }
        return ($text | ConvertFrom-Json)
    }
    function Assert-ArpRejected([string[]]$Command) {
        $info = New-Object System.Diagnostics.ProcessStartInfo
        $info.FileName = $binary
        $info.Arguments = 'documents ' + (($Command + @('--root', $project) | ForEach-Object { '"' + $_ + '"' }) -join ' ')
        $info.UseShellExecute = $false
        $info.CreateNoWindow = $true
        $info.RedirectStandardError = $true
        $process = [System.Diagnostics.Process]::Start($info)
        $errorText = $process.StandardError.ReadToEnd()
        $process.WaitForExit()
        if ($process.ExitCode -ne 2) { throw "Expected rejection: $Command ($errorText)" }
    }
    & $binary documents init --root $project | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Document initialization failed' }
    $candidate = Invoke-ArpJson @('import', $source, '--id', 'smoke')
    Assert-ArpRejected @('adopt', $candidate.proposal_id, '--reviewer', 'synthetic-reviewer')
    $prompt = Join-Path $project 'prompt.txt'
    [System.IO.File]::WriteAllText($prompt, 'Synthetic smoke fixture; no LLM formation was performed.')
    Invoke-ArpJson @('record', $candidate.proposal_id, '--model', 'test-fixture', '--actor', 'synthetic', '--prompt', $prompt) | Out-Null
    $authority = Invoke-ArpJson @('adopt', $candidate.proposal_id, '--reviewer', 'synthetic-reviewer')
    $content = Join-Path $authority.document 'content/Data.yml'
    $before = [System.IO.File]::ReadAllText($content)
    $after = [regex]::Replace($before, '(?m)^(\s+B: )10\r?$', '${1}12')
    if ($before -eq $after) { throw 'Expected editable B1 value was not found' }
    [System.IO.File]::WriteAllText($content, $after, (New-Object System.Text.UTF8Encoding($false)))
    $diff = Invoke-ArpJson @('diff', '--document', 'smoke', '--format', 'json')
    if (@($diff.comparisons[0].changes | Where-Object { $_.before -eq 10 -and $_.after -eq 12 }).Count -ne 1) { throw 'Typed difference was not reported' }
    $resultFile = Join-Path $project '.arp/out/result.xlsx'
    Assert-ArpRejected @('export', 'smoke', '--out', $resultFile)
    if (Test-Path -LiteralPath $resultFile) { throw 'Unreviewed output was created' }
    Invoke-ArpJson @('review', 'smoke', '--reviewer', 'synthetic-reviewer') | Out-Null
    $resultFile = Join-Path $project '.arp/out/result.xlsx'
    $export = Invoke-ArpJson @('export', 'smoke', '--out', $resultFile)
    if (-not $export.written) { throw 'Excel output was not written' }
    $book = [System.IO.Compression.ZipFile]::OpenRead($resultFile)
    try {
        $reader = New-Object System.IO.StreamReader($book.GetEntry('xl/worksheets/sheet1.xml').Open())
        try { [xml]$sheet = $reader.ReadToEnd() } finally { $reader.Dispose() }
        $ns = New-Object System.Xml.XmlNamespaceManager($sheet.NameTable)
        $ns.AddNamespace('s', 'http://schemas.openxmlformats.org/spreadsheetml/2006/main')
        if ($sheet.SelectSingleNode('//s:c[@r="B1"]/s:v', $ns).InnerText -ne '12') { throw 'Independent Excel readback failed' }
    } finally { $book.Dispose() }
    $outputHash = (Get-FileHash -LiteralPath $resultFile -Algorithm SHA256).Hash
    Assert-ArpRejected @('export', 'smoke', '--out', $resultFile)
    if ((Get-FileHash -LiteralPath $resultFile -Algorithm SHA256).Hash -ne $outputHash) { throw 'Existing output changed' }
    $managedSource = Join-Path $project '.arp/originals/source.xlsx/original/source.xlsx'
    [System.IO.File]::AppendAllText($managedSource, 'changed source')
    $staleOutput = Join-Path $project '.arp/out/stale.xlsx'
    Assert-ArpRejected @('export', 'smoke', '--out', $staleOutput)
    if (Test-Path -LiteralPath $staleOutput) { throw 'Stale source was exported' }
    Write-Output "Native Excel and skill smoke passed with empty PATH: $testRoot"
} finally { $env:PATH = $savedPath }
