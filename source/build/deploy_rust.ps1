param(
    [Parameter(Mandatory = $true)][string]$Zip,
    [string]$Destination = ''
)
$ErrorActionPreference = 'Stop'
$repo = [System.IO.Path]::GetFullPath((Split-Path $PSScriptRoot -Parent))
if (-not $Destination) { $Destination = Join-Path (Split-Path $repo -Parent) 'arp4-publish' }
$dest = [System.IO.Path]::GetFullPath($Destination).TrimEnd('\', '/')
if ($dest -eq $repo -or $dest.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase) -or $repo.StartsWith($dest + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Distribution must not overlap source repository' }
function Checked-Target([string]$relative) {
    if ([System.IO.Path]::IsPathRooted($relative) -or $relative.Contains(':') -or @($relative -split '[/\\]' | Where-Object { $_ -eq '..' }).Count -gt 0) { throw "Invalid archive path: $relative" }
    $target = [System.IO.Path]::GetFullPath((Join-Path $dest $relative))
    if (-not $target.StartsWith($dest + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Outside destination: $target" }
    $cursor = $target
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            if ((Get-Item -LiteralPath $cursor -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) { throw "Linked deployment path: $cursor" }
        }
        $parent = Split-Path $cursor -Parent
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
    return $target
}
$archive = (Resolve-Path -LiteralPath $Zip).Path
$expected = (Get-Content -LiteralPath "$archive.sha256" -Raw).Split(' ')[0]
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'ZIP checksum mismatch' }
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zipFile = [System.IO.Compression.ZipFile]::OpenRead($archive)
try {
    $names = @{}
    foreach ($entry in $zipFile.Entries) {
        if (-not $entry.Name) { continue }
        $name = $entry.FullName.Replace('\', '/')
        if ($name.StartsWith('.git/') -or $name.StartsWith('.arp/')) { throw 'Distribution cannot contain project metadata' }
        if ($names.ContainsKey($name)) { throw "Duplicate archive path: $name" }
        $names[$name] = Checked-Target $name
    }
} finally { $zipFile.Dispose() }
$marker = Checked-Target '.arp/rust-distribution.json'
$stage = Checked-Target ('.arp/rust-staging/' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $stage | Out-Null
[System.IO.Compression.ZipFile]::ExtractToDirectory($archive, $stage)
$plan = @()
$hashes = [ordered]@{}
foreach ($name in $names.Keys | Sort-Object) {
    $source = Join-Path $stage $name
    $target = Checked-Target $name
    $hash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    $hashes[$name] = $hash
    if (Test-Path -LiteralPath $target) {
        if (-not (Test-Path -LiteralPath $target -PathType Leaf) -or (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() -ne $hash) { throw "Existing distribution file differs; preserve it before replacing: $target" }
    } else { $plan += [pscustomobject]@{ Source = $source; Target = $target; Hash = $hash } }
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
$record = [ordered]@{ schema_version = '1'; zip_sha256 = $expected; files = $hashes }
$markerText = $record | ConvertTo-Json -Depth 5
if (Test-Path -LiteralPath $marker) {
    if ([System.IO.File]::ReadAllText($marker) -ne $markerText) { throw 'Existing Rust distribution record differs; preserve the previous distribution first' }
} else {
    $stagedMarker = Join-Path $stage 'deployment-record.json'
    [System.IO.File]::WriteAllText($stagedMarker, $markerText, $utf8)
    $plan += [pscustomobject]@{Source = $stagedMarker; Target = $marker; Hash = (Get-FileHash -LiteralPath $stagedMarker -Algorithm SHA256).Hash.ToLowerInvariant()}
}
$installed = @()
try {
    foreach ($item in $plan) {
        New-Item -ItemType Directory -Force -Path (Split-Path $item.Target -Parent) | Out-Null
        [System.IO.File]::Move($item.Source, $item.Target)
        $installed += $item
    }
} catch {
    foreach ($item in $installed) {
        # Every target was resolved and verified beneath the explicit destination above.
        if ((Test-Path -LiteralPath $item.Target -PathType Leaf) -and (Get-FileHash -LiteralPath $item.Target -Algorithm SHA256).Hash.ToLowerInvariant() -eq $item.Hash) {
            Remove-Item -LiteralPath $item.Target
        }
    }
    throw
}
foreach ($name in $hashes.Keys) {
    if ((Get-FileHash -LiteralPath $names[$name] -Algorithm SHA256).Hash.ToLowerInvariant() -ne $hashes[$name]) { throw "Deployed file verification failed: $name" }
}
Write-Output "Rust distribution expanded and verified: $dest"
