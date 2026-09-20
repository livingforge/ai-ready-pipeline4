param([Parameter(Mandatory = $true)][string]$Destination)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
if (Test-Path -LiteralPath $Destination) { throw 'Source snapshot destination already exists' }
New-Item -ItemType Directory -Path $Destination | Out-Null
Push-Location $repo
try {
    $commit = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify source commit' }
    $status = @(& git -c core.quotepath=false status --porcelain --untracked-files=all)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify source changes' }
    $paths = @(& git -c core.quotepath=false ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot enumerate source files' }
    foreach ($path in $paths | Sort-Object -Unique) {
        if ($path -notmatch '^((Cargo\.(toml|lock)|rust-toolchain\.toml|LICENSE|README\.md|\.gitattributes|\.gitignore)$|\.cargo/|crates/|contracts/|surface/|build/|docs/|tests/|examples/|third-party/|\.claude/skills/|\.github/(skills|workflows)/)') { continue }
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
        $cursor = Get-Item -LiteralPath $path -Force
        while ($cursor.FullName -ne $repo) {
            if ($cursor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Linked source path: $path" }
            if ($cursor -is [IO.FileInfo]) { $cursor = $cursor.Directory } else { $cursor = $cursor.Parent }
            if (-not $cursor) { throw "Source outside repository: $path" }
        }
        $target = Join-Path $Destination $path
        New-Item -ItemType Directory -Force -Path (Split-Path $target -Parent) | Out-Null
        Copy-Item -LiteralPath $path -Destination $target
    }
    [ordered]@{ commit = $commit; dirty = ($status.Count -gt 0); changes = $status }
} finally { Pop-Location }
