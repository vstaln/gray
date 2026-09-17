# Hermetic real-installer tests; works with both Windows PowerShell 5.1 and pwsh.
param([Parameter(Mandatory=$true)][string]$Binary)
$ErrorActionPreference = 'Stop'
$installer = Join-Path $PSScriptRoot '..\install-native.ps1'
. $installer
$root = Join-Path ([IO.Path]::GetTempPath()) ('gray-installer-test-' + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($root) | Out-Null
$install = Join-Path $root 'space café\bin'
$archive = Join-Path $root 'fixture.zip'
$payload = Join-Path $root 'payload'
[IO.Directory]::CreateDirectory($payload) | Out-Null
Copy-Item -LiteralPath $Binary -Destination (Join-Path $payload 'gray.exe')
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '..\..\LICENSE') -Destination $payload
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '..\..\THIRD_PARTY_NOTICES.md') -Destination $payload
Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $archive
$digest = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
function Expect-Failure([scriptblock]$Action, [string]$Message) {
    $failed = $false
    try { & $Action } catch {
        $failed = $true
        if ($_.Exception.Message -notlike "*$Message*") { throw "Wrong failure: $_ (expected $Message)" }
    }
    if (-not $failed) { throw "Expected failure: $Message" }
}
try {
    $pathBefore = [Environment]::GetEnvironmentVariable('Path', 'User')
    & $installer -ArchivePath $archive -Sha256 $digest -InstallDir $install -NoPath
    # Public entry point must route native installs without invoking WSL.
    $entry = Join-Path $PSScriptRoot '..\install.ps1'
    & $entry -Native -ArchivePath $archive -Sha256 $digest -InstallDir $install -NoPath
    Expect-Failure { & $entry -Native -ArchivePath $archive -Sha256 ('0' * 64) -InstallDir $install -NoPath } 'checksum mismatch'
    Expect-Failure { & $entry -Native -Wsl } 'parameter set'
    $isolated = Join-Path $root 'entry-only'
    [IO.Directory]::CreateDirectory($isolated) | Out-Null
    Copy-Item -LiteralPath $entry -Destination (Join-Path $isolated 'install.ps1')
    Expect-Failure { & (Join-Path $isolated 'install.ps1') -Native } 'Missing install-native.ps1'
    $installed = Join-Path $install 'gray.exe'
    $before = (Get-FileHash -LiteralPath $installed).Hash
    # Reinstall uses the existing-file replacement path, stable and beta alike.
    & $installer -ArchivePath $archive -Sha256 $digest -InstallDir $install -NoPath -Channel stable
    if ((Get-FileHash -LiteralPath $installed).Hash -ne $before) { throw 'Reinstall changed binary bytes' }
    if ([Environment]::GetEnvironmentVariable('Path', 'User') -cne $pathBefore) { throw '-NoPath modified user PATH' }
    Expect-Failure { & $installer -ArchivePath $archive -Sha256 ('0' * 64) -InstallDir $install -NoPath } 'checksum mismatch'
    Expect-Failure { & $installer -ArchivePath $archive -InstallDir $install -NoPath } 'requires -Sha256'
    # FileShare.None models another process holding the executable open.
    $held = [IO.File]::Open($installed, 'Open', 'Read', 'None')
    try { Expect-Failure { & $installer -ArchivePath $archive -Sha256 $digest -InstallDir $install -NoPath } 'Close all Gray' }
    finally { $held.Dispose() }
    if ((Get-FileHash -LiteralPath $installed).Hash -ne $before) { throw 'Failure lost old executable' }
    $held = [IO.File]::Open((Join-Path $install '.install.lock'), 'Open', 'ReadWrite', 'None')
    try { Expect-Failure { & $installer -ArchivePath $archive -Sha256 $digest -InstallDir $install -NoPath } 'Another install' }
    finally { $held.Dispose() }
    $bad = Join-Path $root 'bad.zip'
    $zip = [IO.Compression.ZipFile]::Open($bad, 'Create')
    try { $zip.CreateEntry('../escape') | Out-Null } finally { $zip.Dispose() }
    $badDigest = (Get-FileHash -LiteralPath $bad -Algorithm SHA256).Hash
    Expect-Failure { & $installer -ArchivePath $bad -Sha256 $badDigest -InstallDir $install -NoPath } 'Unexpected or duplicate'
    if (Test-Path (Join-Path $root 'escape')) { throw 'Archive escaped staging directory' }
    if ((Add-GrayPath 'C:\First;C:\GRAY\bin;C:\Last' 'c:\gray\bin\') -cne 'C:\First;C:\GRAY\bin;C:\Last') { throw 'Duplicate PATH entry' }
    if ((Add-GrayPath 'C:\First' 'C:\gray\bin') -cne 'C:\First;C:\gray\bin') { throw 'PATH append failed' }
    Write-Host 'PASS: install, repeat, channels, checksums, locks, traversal, PATH, preserved binary'
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
