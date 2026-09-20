param(
    [Parameter(Mandatory = $true)][string]$Distribution,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$Cargo = 'cargo'
)
$ErrorActionPreference = 'Stop'
$dist = (Resolve-Path -LiteralPath $Distribution).Path
& "$PSScriptRoot/verify_audit.ps1" -Distribution $dist
$source = Join-Path $dist 'source'
$releasePath = Join-Path $dist 'audit/release.json'
$release = Get-Content -LiteralPath $releasePath -Raw -Encoding UTF8 | ConvertFrom-Json
$output = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $output | Out-Null
$zip = Join-Path $output "arp4-v$($release.version)-offline-dependencies.zip"
if ((Test-Path -LiteralPath $zip) -or (Test-Path -LiteralPath "$zip.sha256")) { throw 'Offline output already exists' }
$stage = Join-Path $output "offline-stage-$([guid]::NewGuid().ToString('N'))"
$offline = Join-Path $stage 'offline'
New-Item -ItemType Directory -Path (Join-Path $offline 'audit') -Force | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)
Push-Location $source
try {
    $config = @(& $Cargo vendor --locked (Join-Path $offline 'vendor'))
    if ($LASTEXITCODE -ne 0) { throw 'Cargo vendor failed' }
    $configText = ($config -join "`n") -replace '(?m)^directory = .+$', 'directory = "offline/vendor"'
    [IO.File]::WriteAllText((Join-Path $offline 'vendor-config.toml'), ($configText + "`n"), $utf8)
} finally { Pop-Location }
$record = @{ version = $release.version; standard_release_sha256 = (Get-FileHash -LiteralPath $releasePath -Algorithm SHA256).Hash.ToLowerInvariant() }
[IO.File]::WriteAllText((Join-Path $offline 'release.json'), ($record | ConvertTo-Json), $utf8)
$sums = @(Get-ChildItem -LiteralPath $offline -Recurse -Force -File | Sort-Object FullName | ForEach-Object {
    $relative = $_.FullName.Substring($offline.Length + 1).Replace('\', '/')
    "$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant())  $relative"
})
[IO.File]::WriteAllText((Join-Path $offline 'audit/SHA256SUMS'), (($sums -join "`n") + "`n"), $utf8)
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip)
[IO.File]::WriteAllText("$zip.sha256", "$((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant())  $([IO.Path]::GetFileName($zip))`n", $utf8)
Write-Output $zip
