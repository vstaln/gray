# gray installer for Windows — https://gray.alignment.id
# Native is the default route: no WSL, no Linux distro, no elevation.
# Pass -Wsl for the compatibility route that installs the Linux build inside WSL.
# Download and inspect scripts rather than piping installation to iex.
[CmdletBinding(DefaultParameterSetName='Native')]
param(
    # Accepted for explicitness and back-compatibility; native is already the
    # default, so this switch changes nothing on its own.
    [Parameter(ParameterSetName='Native')][switch]$Native,
    [Parameter(Mandatory=$true, ParameterSetName='Wsl')][switch]$Wsl,
    [Parameter(ParameterSetName='Native')][ValidateSet('stable', 'beta')][string]$Channel = 'beta',
    [Parameter(ParameterSetName='Native')][string]$InstallDir = $env:GRAY_INSTALL_DIR,
    [Parameter(ParameterSetName='Native')][switch]$NoPath,
    [Parameter(ParameterSetName='Native')][string]$ArchivePath,
    [Parameter(ParameterSetName='Native')][string]$Sha256,
    [Parameter(ParameterSetName='Native')][uri]$BaseUri = 'https://gray.alignment.id/dl/'
)

$ErrorActionPreference = 'Stop'
if ($PSCmdlet.ParameterSetName -ne 'Wsl') {
    # Native is the default route. Reuse the tested native implementation and
    # never fall back to WSL on failure.
    if (-not $PSScriptRoot) { throw 'Save install.ps1 and install-native.ps1 together before running the native install.' }
    $installer = Join-Path $PSScriptRoot 'install-native.ps1'
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
        throw 'Missing install-native.ps1. Extract both installer scripts from the release artifact.'
    }
    & $installer -Channel $Channel -InstallDir $InstallDir -NoPath:$NoPath -ArchivePath $ArchivePath -Sha256 $Sha256 -BaseUri $BaseUri
    return
}

# Compatibility route, requested explicitly with -Wsl.

Write-Host ""
Write-Host "  gray installer" -ForegroundColor Cyan
Write-Host "  --------------"

# 1. WSL present?
$wsl = Get-Command wsl.exe -ErrorAction SilentlyContinue
if (-not $wsl) {
    Write-Host ""
    Write-Host "  -Wsl was requested, and WSL is not installed." -ForegroundColor Yellow
    Write-Host "  Install it with:   wsl --install" -ForegroundColor Yellow
    Write-Host "  (reboot, then re-run this installer)"
    Write-Host "  Or drop -Wsl: gray installs natively on Windows without it." -ForegroundColor Yellow
    exit 1
}

# 2. A distro installed?
$distros = & wsl.exe -l -q 2>$null | Where-Object { $_ -and ($_ -replace "`0","").Trim() }
if (-not $distros) {
    Write-Host "  No WSL distro found. Installing Ubuntu (default)..."
    & wsl.exe --install -d Ubuntu
    Write-Host "  Reboot if prompted, then re-run this installer."
    exit 0
}

Write-Host "  -> installing gray into WSL ($($distros[0]))..."

# 3. Run the sh installer inside WSL
& wsl.exe -e sh -c "curl -fsSL https://gray.alignment.id/install.sh | sh"
if ($LASTEXITCODE -ne 0) {
    # curl missing inside the distro? install it then retry once
    & wsl.exe -e sh -c "sudo apt-get update -qq && sudo apt-get install -y -qq curl >/dev/null && curl -fsSL https://gray.alignment.id/install.sh | sh"
}

Write-Host ""
Write-Host "  Done. To use gray:" -ForegroundColor Green
Write-Host "    1. open a WSL terminal (or: wsl)"
Write-Host "    2. export GRAY_API_KEY=sk-or-...      # openrouter.ai / deepseek.com key"
Write-Host "    3. export GRAY_MODEL=deepseek/deepseek-chat"
Write-Host "    4. run: gray"
