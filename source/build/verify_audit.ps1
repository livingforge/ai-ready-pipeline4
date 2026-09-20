param([Parameter(Mandatory = $true)][string]$Distribution)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $Distribution).Path.TrimEnd('\', '/')
$manifest = Join-Path $root 'audit/SHA256SUMS'
$seen = @{}
foreach ($line in [IO.File]::ReadAllLines($manifest)) {
    if ($line -notmatch '^([0-9a-f]{64})  (.+)$') { throw 'Invalid checksum entry' }
    $hash = $Matches[1]; $relative = $Matches[2]
    if ([IO.Path]::IsPathRooted($relative) -or $relative.Contains(':') -or @($relative -split '[/\\]' | Where-Object { $_ -eq '..' }).Count -gt 0) { throw 'Invalid checksum path' }
    $path = [IO.Path]::GetFullPath((Join-Path $root $relative))
    if (-not $path.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase) -or $seen.ContainsKey($path)) { throw 'Duplicate or outside checksum path' }
    $seen[$path] = $true
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $hash) { throw "Checksum mismatch: $relative" }
}
if ($seen.Count -eq 0) { throw 'Empty checksum manifest' }
Write-Output "Verified $($seen.Count) distribution files. Additional local files are not checked."
