# gray installer for Windows — https://gray.alignment.id
# iwr https://gray.alignment.id/install.ps1 -UseBasicParsing | iex
#
# Windows native builds are not packaged yet, so this drives the Linux binary
# through WSL (Windows Subsystem for Linux) — which gives you a full gray REPL.

$ErrorActionPreference = "Stop"

Write-Host ""
Write-Host "  gray installer" -ForegroundColor Cyan
Write-Host "  --------------"

# 1. WSL present?
$wsl = Get-Command wsl.exe -ErrorAction SilentlyContinue
if (-not $wsl) {
    Write-Host ""
    Write-Host "  WSL is not installed. gray runs inside WSL on Windows." -ForegroundColor Yellow
    Write-Host "  Install it with:   wsl --install" -ForegroundColor Yellow
    Write-Host "  (reboot, then re-run this installer)"
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
