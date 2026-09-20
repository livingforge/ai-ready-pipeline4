param(
    [Parameter(Mandatory = $true)][string]$Zip,
    [Parameter(Mandatory = $true)][string]$OfflineZip,
    [string]$Cargo = 'cargo'
)
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) "arp-offline-tests-$([guid]::NewGuid().ToString('N'))"
Add-Type -AssemblyName System.IO.Compression.FileSystem
foreach ($archive in @($Zip, $OfflineZip)) {
    $archive = (Resolve-Path -LiteralPath $archive).Path
    $expected = ([IO.File]::ReadAllText("$archive.sha256")).Split(' ')[0]
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'ZIP checksum mismatch' }
    [IO.Compression.ZipFile]::ExtractToDirectory($archive, $root)
}
& "$PSScriptRoot/verify_audit.ps1" -Distribution $root
& "$PSScriptRoot/verify_audit.ps1" -Distribution (Join-Path $root 'offline')
$binding = Get-Content -LiteralPath (Join-Path $root 'offline/release.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if ($binding.standard_release_sha256 -ne (Get-FileHash -LiteralPath (Join-Path $root 'audit/release.json') -Algorithm SHA256).Hash.ToLowerInvariant()) { throw 'Offline package belongs to a different release' }
$savedHome = $env:CARGO_HOME
$savedTarget = $env:CARGO_TARGET_DIR
$env:CARGO_HOME = Join-Path $root 'empty-cargo-home'
$env:CARGO_TARGET_DIR = Join-Path $root 'build-output'
Push-Location (Join-Path $root 'source')
try {
    & $Cargo --config ../offline/vendor-config.toml test --workspace --frozen
    if ($LASTEXITCODE -ne 0) { throw 'Offline source tests failed' }
    & $Cargo --config ../offline/vendor-config.toml build --release --frozen --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Offline release build failed' }
} finally {
    Pop-Location
    $env:CARGO_HOME = $savedHome
    $env:CARGO_TARGET_DIR = $savedTarget
}
Write-Output "Offline dependency rebuild and tests passed with an empty Cargo home: $root"
