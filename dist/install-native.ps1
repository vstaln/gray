# Experimental native Windows installer. The public install.ps1 still uses WSL
# until clean-machine, credential ACL, and full release acceptance checks pass.
# Download and inspect this script; no elevation or execution-policy changes.
[CmdletBinding()]
param(
    [ValidateSet('stable', 'beta')][string]$Channel = 'beta',
    [string]$InstallDir = $env:GRAY_INSTALL_DIR,
    [switch]$NoPath,
    # Offline artifacts let CI test the real installer without a live release.
    [string]$ArchivePath,
    [string]$Sha256,
    [uri]$BaseUri = 'https://gray.alignment.id/dl/'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Add-GrayPath([string]$Current, [string]$Directory) {
    $wanted = $Directory.TrimEnd('\', '/')
    foreach ($entry in ($Current -split ';')) {
        if ([string]::Equals($entry.Trim().Trim('"').TrimEnd('\', '/'), $wanted,
                [StringComparison]::OrdinalIgnoreCase)) { return $Current }
    }
    if ([string]::IsNullOrEmpty($Current)) { return $Directory }
    return $Current.TrimEnd(';') + ';' + $Directory
}

function Assert-GrayVersion([string]$Executable) {
    # Bound the probe, including corrupt binaries that start but never finish.
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $Executable
    $info.Arguments = '--version'
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $info
    try {
        if (-not $process.Start()) { throw 'Could not start version probe' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(10000)) {
            $process.Kill()
            throw 'Version probe timed out'
        }
        if ($process.ExitCode -ne 0 -or $stdout.Result.Trim() -notmatch '^gray \d+\.\d+\.\d+') {
            throw 'Candidate is not a working gray executable'
        }
        return $stdout.Result.Trim()
    } finally { $process.Dispose() }
}

function Install-NativeGray {
    if ($env:OS -ne 'Windows_NT') { throw 'Native installer requires Windows' }
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne 'AMD64') { throw "Unsupported native architecture: $arch (x64 required; use WSL)" }
    if ([Environment]::OSVersion.Version.Build -lt 22000) { throw 'Native preview requires Windows 11 or newer' }
    if (-not $InstallDir) {
        if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is missing; specify -InstallDir' }
        $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\gray\bin'
    }
    $destination = [IO.Path]::GetFullPath($InstallDir)
    $binary = Join-Path $destination 'gray.exe'
    $name = "gray-$Channel-x86_64-windows.zip"
    if ($ArchivePath -and $Sha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw 'Offline installation requires -Sha256 with the published archive digest'
    }
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('gray-install-' + [guid]::NewGuid().ToString('N'))
    $stage = $null
    $lock = $null
    $backup = $null
    try {
        [IO.Directory]::CreateDirectory($temp) | Out-Null
        $archive = Join-Path $temp $name
        if ($ArchivePath) {
            Copy-Item -LiteralPath $ArchivePath -Destination $archive
        } else {
            if ($BaseUri.Scheme -ne 'https') { throw 'Downloads require HTTPS; use -ArchivePath for offline tests' }
            # PS 5.1 may otherwise negotiate old TLS. Never disable validation.
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            $base = $BaseUri.AbsoluteUri.TrimEnd('/') + '/'
            Invoke-WebRequest -UseBasicParsing -Uri ($base + $name) -OutFile $archive -TimeoutSec 60
            $sumFile = Join-Path $temp 'checksum'
            Invoke-WebRequest -UseBasicParsing -Uri ($base + $name + '.sha256') -OutFile $sumFile -TimeoutSec 60
            $lines = @(Get-Content -LiteralPath $sumFile | Where-Object { $_.Trim() })
            $pattern = '^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($name) + '$'
            if ($lines.Count -ne 1 -or $lines[0] -notmatch $pattern) { throw 'Missing or invalid archive checksum' }
            $Sha256 = $Matches[1]
        }
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $Sha256) {
            throw 'Archive checksum mismatch; installation unchanged'
        }
        # A checksum detects corruption, not independent publisher authenticity.
        # Strict flat allowlist prevents ZIP traversal, links, alternate streams,
        # duplicate case-insensitive names, and unbounded extraction.
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            $seen = @{}
            $total = [long]0
            foreach ($entry in $zip.Entries) {
                if ($entry.FullName -cnotin @('gray.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md') -or $seen.ContainsKey($entry.FullName)) {
                    throw "Unexpected or duplicate ZIP member: $($entry.FullName)"
                }
                $seen[$entry.FullName] = $true
                $total += $entry.Length
                if ($total -gt 268435456) { throw 'Expanded archive exceeds 256 MiB' }
            }
            if (-not $seen.ContainsKey('gray.exe') -or -not $seen.ContainsKey('LICENSE') -or -not $seen.ContainsKey('THIRD_PARTY_NOTICES.md')) {
                throw 'Archive must include gray.exe and license notices'
            }
            foreach ($entry in $zip.Entries) {
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $temp $entry.FullName))
            }
        } finally { $zip.Dispose() }
        $version = Assert-GrayVersion (Join-Path $temp 'gray.exe')
        [IO.Directory]::CreateDirectory($destination) | Out-Null
        # Serialize installers in this destination, not in a possibly different
        # GRAY_HOME. Leave the lock file behind; deleting it races another opener.
        try { $lock = [IO.File]::Open((Join-Path $destination '.install.lock'), 'OpenOrCreate', 'ReadWrite', 'None') }
        catch { throw 'Another install is active or the destination is not writable' }
        $stage = Join-Path $destination ('.gray-' + [guid]::NewGuid().ToString('N') + '.exe')
        Copy-Item -LiteralPath (Join-Path $temp 'gray.exe') -Destination $stage
        foreach ($notice in @('LICENSE', 'THIRD_PARTY_NOTICES.md')) {
            Copy-Item -LiteralPath (Join-Path $temp $notice) -Destination (Join-Path $destination $notice) -Force
        }
        $backup = Join-Path $destination ('.gray-backup-' + [guid]::NewGuid().ToString('N') + '.exe')
        $hadBinary = [IO.File]::Exists($binary)
        try {
            # Same-volume replacement: preserve the original on sharing errors.
            # Do not try to replace the running gray.exe by elevation/reboot.
            if ($hadBinary) { [IO.File]::Replace($stage, $binary, $backup) }
            else { [IO.File]::Move($stage, $binary) }
        } catch { throw 'Cannot replace gray.exe. Close all Gray sessions and retry; check destination permissions.' }
        try { $version = Assert-GrayVersion $binary }
        catch {
            if ($hadBinary) { [IO.File]::Replace($backup, $binary, $null) }
            else { [IO.File]::Delete($binary) }
            throw
        }
        if ([IO.File]::Exists($backup)) { [IO.File]::Delete($backup) }
        if (-not $NoPath) {
            try {
                $path = [Environment]::GetEnvironmentVariable('Path', 'User')
                [Environment]::SetEnvironmentVariable('Path', (Add-GrayPath $path $destination), 'User')
                Write-Host 'Open a new terminal for the user PATH change.'
            } catch { Write-Warning "Binary installed, but user PATH was not updated. Run: $binary" }
        }
        Write-Host "Installed $version (experimental native $Channel): $binary"
        Write-Host 'Shell commands require Git for Windows. Native release acceptance is still pending.'
        Write-Host 'Uninstall: close Gray, remove this install folder and its user PATH entry; keep your .gray data.'
    } finally {
        if ($stage -and [IO.File]::Exists($stage)) { [IO.File]::Delete($stage) }
        if ($lock) { $lock.Dispose() }
        # Never delete an unrecovered backup: retain it for manual recovery.
        if ($backup -and [IO.File]::Exists($backup)) { Write-Warning "Previous binary retained at $backup" }
        if ([IO.Directory]::Exists($temp)) { Remove-Item -LiteralPath $temp -Recurse -Force }
    }
}

# Dot-source exposes pure helpers to tests without performing an installation.
if ($MyInvocation.InvocationName -ne '.') { Install-NativeGray }
