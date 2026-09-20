param([Parameter(Mandatory = $true)][string]$Zip)
$ErrorActionPreference = 'Stop'
$archive = (Resolve-Path -LiteralPath $Zip).Path
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "arp-deploy-tests-$([guid]::NewGuid().ToString('N'))"
$dest = Join-Path $testRoot 'distribution'
New-Item -ItemType Directory -Path (Join-Path $dest '.git') -Force | Out-Null
$keep = Join-Path $dest '.git/keep.txt'
[IO.File]::WriteAllText($keep, 'keep')
& "$PSScriptRoot/deploy_rust.ps1" -Zip $archive -Destination $dest
& "$PSScriptRoot/deploy_rust.ps1" -Zip $archive -Destination $dest
if ([IO.File]::ReadAllText($keep) -ne 'keep') { throw 'Git metadata changed' }
if (-not (Test-Path -LiteralPath (Join-Path $dest 'arp4.exe'))) { throw 'Missing deployed binary' }
$conflict = Join-Path $testRoot 'conflict'
New-Item -ItemType Directory -Path $conflict | Out-Null
[IO.File]::WriteAllText((Join-Path $conflict 'arp4.exe'), 'user file')
$rejected = $false
try { & "$PSScriptRoot/deploy_rust.ps1" -Zip $archive -Destination $conflict } catch { $rejected = $true }
if (-not $rejected -or (Test-Path -LiteralPath (Join-Path $conflict 'README.md'))) { throw 'Conflict did not reject the whole deployment' }
if ([IO.File]::ReadAllText((Join-Path $conflict 'arp4.exe')) -ne 'user file') { throw 'User file changed' }
$badZip = Join-Path $testRoot 'bad.zip'
Copy-Item -LiteralPath $archive -Destination $badZip
[IO.File]::WriteAllText("$badZip.sha256", ('0' * 64) + '  bad.zip')
$badDest = Join-Path $testRoot 'bad-destination'
$rejected = $false
try { & "$PSScriptRoot/deploy_rust.ps1" -Zip $badZip -Destination $badDest } catch { $rejected = $true }
if (-not $rejected -or (Test-Path -LiteralPath $badDest)) { throw 'Invalid checksum changed destination' }
Write-Output "Deployment tests passed: $testRoot"
