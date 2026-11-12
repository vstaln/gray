[CmdletBinding()]
param(
    [ValidateSet('stable', 'beta')][string]$Channel = 'stable',
    [string]$InstallDir = $env:GRAY_INSTALL_DIR,
    [switch]$NoPath,

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

function Get-GrayDigest([string[]]$Lines, [string]$Name) {

    $pattern = '^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($Name) + '$'
    $found = @($Lines | ForEach-Object { $_.Trim() } | Where-Object { $_ -match $pattern })
    if ($found.Count -ne 1) { throw 'Missing or invalid archive checksum' }
    $null = $found[0] -match $pattern
    return $Matches[1]
}

function Assert-GrayVersion([string]$Executable) {

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
    if ($arch -ne 'AMD64') { throw "Unsupported native architecture: $arch (x64 required)" }
    if ([Environment]::OSVersion.Version.Build -lt 10240) { throw 'Native install requires Windows 10 or newer' }
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

            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            $base = $BaseUri.AbsoluteUri.TrimEnd('/') + '/'
            Invoke-WebRequest -UseBasicParsing -Uri ($base + $name) -OutFile $archive -TimeoutSec 60

            $sumFile = Join-Path $temp 'checksum'
            Invoke-WebRequest -UseBasicParsing -Uri ($base + "SHA256SUMS-$Channel") -OutFile $sumFile -TimeoutSec 60
            $Sha256 = Get-GrayDigest @(Get-Content -LiteralPath $sumFile) $name
        }
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $Sha256) {
            throw 'Archive checksum mismatch; installation unchanged'
        }

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
        Write-Host "Installed $version (native $Channel): $binary"
        Write-Host 'Shell commands require Git for Windows (Git Bash). Gray does not install it or WSL.'
        Write-Host 'Uninstall: close Gray, remove this install folder and its user PATH entry; keep your .gray data.'
        $flavor = 'core'
        if ($env:GRAY_INSTALL_PLUGINS -ne '0') {
            $flavor = 'full'
            $plugins = if ($env:GRAY_PLUGINS) { $env:GRAY_PLUGINS -split '\s+' | Where-Object { $_ } } else { @('account', 'memory', 'search', 'skills', 'voice', 'questions', 'subagents', 'mcp', 'gateway') }
            $failed = @()
            foreach ($plugin in $plugins) {
                try {
                    & $binary plugin install --yes $plugin | Out-Null
                    if ($LASTEXITCODE -eq 0) { Write-Host "Installed gray-$plugin" }
                    else { $failed += $plugin }
                } catch { $failed += $plugin }
            }
            if ($failed.Count -gt 0) {
                Write-Host 'Some plugins could not be installed now -- retry later:'
                foreach ($plugin in $failed) {
                    Write-Host "  gray plugin install --yes $plugin"
                }
            }
        }
        if ($env:GRAY_TELEMETRY -ne '0' -and $BaseUri.Host -eq 'gray.alignment.id') {
            try {
                $v = ($version -split '\s+')[-1]
                $a = if ($env:PROCESSOR_ARCHITECTURE -eq 'AMD64') { 'x86_64' } else { $env:PROCESSOR_ARCHITECTURE.ToLower() }
                $body = @{ os = 'windows'; arch = $a; channel = $Channel; version = $v; flavor = $flavor } | ConvertTo-Json -Compress
                $null = Invoke-RestMethod -Method Post -Uri 'https://gray.alignment.id/api/telemetry/install' -TimeoutSec 4 -ContentType 'application/json' -Body $body
            } catch {}
        }
    } finally {
        if ($stage -and [IO.File]::Exists($stage)) { [IO.File]::Delete($stage) }
        if ($lock) { $lock.Dispose() }

        if ($backup -and [IO.File]::Exists($backup)) { Write-Warning "Previous binary retained at $backup" }
        if ([IO.Directory]::Exists($temp)) { Remove-Item -LiteralPath $temp -Recurse -Force }
    }
}

if ($MyInvocation.InvocationName -ne '.') { Install-NativeGray }
