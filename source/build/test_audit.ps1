param([Parameter(Mandatory = $true)][string]$Zip)
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) "arp-audit-tests-$([guid]::NewGuid().ToString('N'))"
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::ExtractToDirectory((Resolve-Path -LiteralPath $Zip).Path, $root)
& "$PSScriptRoot/verify_audit.ps1" -Distribution $root
foreach ($relative in @('source/Cargo.lock', 'source/.cargo/config.toml', 'source/crates/arp4-cli/tests/excel.rs', 'source/tests/dataset/Java.yml', 'source/surface/skills/arp4/body.md', 'source/.github/skills/arp4/SKILL.md', 'audit/cargo-test.log', 'audit/TESTING.md')) {
    if (-not (Test-Path -LiteralPath (Join-Path $root $relative) -PathType Leaf)) { throw "Audit asset missing: $relative" }
}
if (Test-Path -LiteralPath (Join-Path $root 'source/target')) { throw 'Build outputs leaked into source' }
$release = Get-Content -LiteralPath (Join-Path $root 'audit/release.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if ($release.binary_sha256 -ne (Get-FileHash -LiteralPath (Join-Path $root 'arp4.exe') -Algorithm SHA256).Hash.ToLowerInvariant()) { throw 'Binary provenance mismatch' }
$source = Join-Path $root 'source/crates/arp4-cli/src/main.rs'
[IO.File]::AppendAllText($source, "`n// tampered")
$rejected = $false
try { & "$PSScriptRoot/verify_audit.ps1" -Distribution $root } catch { $rejected = $true }
if (-not $rejected) { throw 'Source tampering was not detected' }
Write-Output 'Audit archive and tamper detection tests passed'
