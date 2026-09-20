param(
    [string]$OutputDirectory = "",
    [string]$Cargo = "cargo"
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repo 'target/preview-distribution' }
$output = [System.IO.Path]::GetFullPath($OutputDirectory)
$stage = Join-Path $repo "target/package-stage/$([guid]::NewGuid().ToString('N'))/distribution"
New-Item -ItemType Directory -Path $stage | Out-Null
$snapshot = Join-Path $stage 'source'
$provenance = & "$PSScriptRoot/audit_source.ps1" -Destination $snapshot
$repo = $snapshot
$savedTarget = $env:CARGO_TARGET_DIR
$buildTarget = if ($savedTarget) { [IO.Path]::GetFullPath($savedTarget) } else { Join-Path $snapshot 'target' }
$env:CARGO_TARGET_DIR = $buildTarget
Push-Location $snapshot
try {
    & $Cargo build --release --locked --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed' }
    $binary = Join-Path $buildTarget 'x86_64-pc-windows-msvc/release/arp4.exe'
    $report = & $binary doctor --format json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Rust capability report failed' }
    $name = "arp4-v$($report.version)-windows-x64-preview"
    New-Item -ItemType Directory -Force -Path $output | Out-Null
    $zip = Join-Path $output "$name.zip"
    if ((Test-Path -LiteralPath $zip) -or (Test-Path -LiteralPath "$zip.sha256")) { throw "Output already exists: $zip" }
    Copy-Item -LiteralPath $binary -Destination (Join-Path $stage 'arp4.exe')
    Copy-Item -LiteralPath (Join-Path $repo 'docs/rust-preview.md') -Destination (Join-Path $stage 'README.md')
    Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $stage
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Join-Path $stage 'capabilities.json'), ($report | ConvertTo-Json -Depth 10), $utf8)
    $metadata = & $Cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
    $notices = @('# Third-party dependencies', '', 'License expressions are from the locked Cargo package metadata. License texts are in licenses/.', '')
    $activePackages = @($metadata.resolve.nodes.id)
    foreach ($package in $metadata.packages | Where-Object { $activePackages -contains $_.id } | Sort-Object name, version) {
        if (-not $package.source) { continue }
        $notices += "- $($package.name) $($package.version): $($package.license)"
        $directory = Split-Path $package.manifest_path -Parent
        $texts = @(Get-ChildItem -LiteralPath $directory -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' })
        if ($texts.Count -eq 0) {
            # Some workspace subcrates omit the repository-wide license from their crate archive.
            # Reuse it only from a package at the identical repository commit and license expression.
            $vcsFile = Join-Path $directory '.cargo_vcs_info.json'
            $commit = if (Test-Path -LiteralPath $vcsFile) { (Get-Content -LiteralPath $vcsFile -Raw | ConvertFrom-Json).git.sha1 } else { $null }
            foreach ($donor in $metadata.packages | Where-Object { $_.repository -eq $package.repository -and $_.license -eq $package.license }) {
                if (-not $commit -or -not $package.repository) { continue }
                $donorDirectory = Split-Path $donor.manifest_path -Parent
                $donorVcs = Join-Path $donorDirectory '.cargo_vcs_info.json'
                if (-not (Test-Path -LiteralPath $donorVcs)) { continue }
                if ((Get-Content -LiteralPath $donorVcs -Raw | ConvertFrom-Json).git.sha1 -ne $commit) { continue }
                $texts = @(Get-ChildItem -LiteralPath $donorDirectory -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' })
                if ($texts.Count -gt 0) {
                    $notices += "  Repository license from $($donor.name) $($donor.version), commit $commit."
                    break
                }
            }
        }
        if ($texts.Count -eq 0 -and $package.repository -eq 'https://github.com/Nugine/simd' -and $commit -eq 'd74c030d9dc4f3cae02146d1f497ff62726ef09a' -and $package.license -eq 'MIT') {
            $texts = @(Get-Item -LiteralPath (Join-Path $repo 'third-party/simd/LICENSE'))
            $notices += "  Repository license vendored from https://github.com/Nugine/simd/tree/$commit (see third-party/simd/README.md in the source)."
        }
        if ($texts.Count -eq 0) { throw "No license texts found for $($package.name)" }
        $destination = Join-Path $stage "licenses/$($package.name)-$($package.version)"
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        foreach ($license in $texts) { Copy-Item -LiteralPath $license.FullName -Destination $destination }
    }
    [System.IO.File]::WriteAllText((Join-Path $stage 'THIRD-PARTY-NOTICES.md'), ($notices -join "`n"), $utf8)
    $audit = Join-Path $stage 'audit'
    New-Item -ItemType Directory -Path $audit | Out-Null
    $ErrorActionPreference = 'Continue'
    & $Cargo test --workspace --locked 2>&1 | ForEach-Object { "$_" } | Out-File -LiteralPath (Join-Path $audit 'cargo-test.log') -Encoding utf8
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw 'Packaged source tests failed; see audit/cargo-test.log' }
    $ErrorActionPreference = 'Continue'
    & $Cargo run --locked --example sync_skills -- --check 2>&1 | ForEach-Object { "$_" } | Out-File -LiteralPath (Join-Path $audit 'skills-check.log') -Encoding utf8
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw 'Packaged skill synchronization check failed' }
    $rustc = & (Join-Path (Split-Path (Get-Command $Cargo).Source -Parent) 'rustc.exe') -vV
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify rustc' }
    $cargoVersion = & $Cargo --version
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify Cargo' }
    $release = [ordered]@{
        schema_version = 1; version = $report.version; source = $provenance
        built_at_utc = [DateTime]::UtcNow.ToString('o')
        target = 'x86_64-pc-windows-msvc'; cargo = $cargoVersion; rustc = $rustc
        build_command = 'cargo build --release --locked --target x86_64-pc-windows-msvc'
        rustflags = $env:RUSTFLAGS; cargo_encoded_rustflags = $env:CARGO_ENCODED_RUSTFLAGS
        binary_sha256 = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
        verification = @{ cargo_test = 'passed'; skills_check = 'passed'; network_isolation = 'not tested'; clean_windows = 'not tested'; reproducible_binary = 'not verified' }
    }
    [IO.File]::WriteAllText((Join-Path $audit 'release.json'), ($release | ConvertTo-Json -Depth 8), $utf8)
    Copy-Item -LiteralPath (Join-Path $repo 'docs/audit-testing.md') -Destination (Join-Path $audit 'TESTING.md')
    [IO.File]::WriteAllText((Join-Path $audit 'verification.md'), "# Verification`n`nSource snapshot build, cargo test and skill synchronization passed. See logs and release.json.`nNetwork isolation, clean Windows, Excel/Agent acceptance and bit-for-bit reproducibility were not tested by this packaging script.`n", $utf8)
    # Build outputs must never be included in the source archive.
    if (-not $savedTarget) {
        $buildTarget = [IO.Path]::GetFullPath($buildTarget)
        if (-not $buildTarget.StartsWith([IO.Path]::GetFullPath($stage) + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe build output path' }
        Remove-Item -LiteralPath $buildTarget -Recurse -Force
    }
    $sums = @(Get-ChildItem -LiteralPath $stage -Recurse -Force -File | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
        "$( (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() )  $relative"
    })
    [IO.File]::WriteAllText((Join-Path $audit 'SHA256SUMS'), (($sums -join "`n") + "`n"), $utf8)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip)
    $checksum = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText("$zip.sha256", "$checksum  $name.zip`n", $utf8)
    Write-Output $zip
} finally { $env:CARGO_TARGET_DIR = $savedTarget; Pop-Location }
