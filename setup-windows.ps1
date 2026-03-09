# Kevlar Windows Setup Script
# Run this in PowerShell as Administrator

Write-Host "=== Kevlar Windows Build Environment Setup ===" -ForegroundColor Cyan

# Check if running as Administrator
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "ERROR: This script must be run as Administrator" -ForegroundColor Red
    Write-Host "Right-click PowerShell and select 'Run as Administrator'" -ForegroundColor Yellow
    exit 1
}

# Check for Chocolatey
Write-Host "`nChecking for Chocolatey..." -ForegroundColor Yellow
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Write-Host "Installing Chocolatey..." -ForegroundColor Yellow
    Set-ExecutionPolicy Bypass -Scope Process -Force
    [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
    Invoke-Expression ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))

    # Refresh environment
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path","User")
} else {
    Write-Host "Chocolatey already installed" -ForegroundColor Green
}

# Install dependencies
Write-Host "`nInstalling build dependencies..." -ForegroundColor Yellow

# Install Visual Studio Build Tools first (required for Rust MSVC linker)
Write-Host "Installing Visual Studio 2022 Build Tools (this may take 10-15 minutes)..." -ForegroundColor Cyan
choco install -y visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"

$packages = @(
    "make",
    "qemu",
    "docker-desktop",
    "rustup.install"
)

foreach ($package in $packages) {
    Write-Host "Installing $package..." -ForegroundColor Cyan
    choco install -y $package
}

Write-Host "`n=== Installation Complete ===" -ForegroundColor Green
Write-Host "`nNext steps:" -ForegroundColor Yellow
Write-Host "1. Add cargo to your system PATH:" -ForegroundColor White
Write-Host "   - Press Win+R, type 'sysdm.cpl', press Enter" -ForegroundColor White
Write-Host "   - Advanced tab -> Environment Variables" -ForegroundColor White
Write-Host "   - Under User variables, select Path -> Edit -> New" -ForegroundColor White
Write-Host "   - Add: %USERPROFILE%\.cargo\bin" -ForegroundColor White
Write-Host "2. Restart your terminal (Git Bash or PowerShell)" -ForegroundColor White
Write-Host "3. Start Docker Desktop from the Start menu" -ForegroundColor White
Write-Host "4. Make sure Docker is in Linux containers mode (right-click Docker tray icon)" -ForegroundColor White
Write-Host "5. Open Git Bash and run:" -ForegroundColor White
Write-Host "   export PATH=`"`$HOME/.cargo/bin:`$PATH`"" -ForegroundColor Cyan
Write-Host "   rustup install nightly" -ForegroundColor Cyan
Write-Host "   rustup default nightly" -ForegroundColor Cyan
Write-Host "   rustup component add rust-src --toolchain nightly" -ForegroundColor Cyan
Write-Host "   cargo install rustfilt" -ForegroundColor Cyan
Write-Host "6. Then run: make run" -ForegroundColor Cyan
Write-Host "`nSee WINDOWS-SETUP.md for more details" -ForegroundColor Yellow
