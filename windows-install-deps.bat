@echo off
REM Kevlar Windows Dependency Installer
REM This script must be run as Administrator

echo ====================================
echo Kevlar Windows Dependency Installer
echo ====================================
echo.

REM Check for admin privileges
net session >nul 2>&1
if %errorLevel% neq 0 (
    echo ERROR: This script must be run as Administrator
    echo Right-click this file and select "Run as administrator"
    echo.
    pause
    exit /b 1
)

echo Installing dependencies via Chocolatey...
echo.

REM Check if Chocolatey is installed
where choco >nul 2>&1
if %errorLevel% neq 0 (
    echo Chocolatey not found. Installing Chocolatey first...
    powershell -NoProfile -ExecutionPolicy Bypass -Command "iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))"

    REM Refresh environment
    call refreshenv
)

echo Installing: make, qemu, docker-desktop, rustup
echo This may take several minutes...
echo.

choco install -y make
choco install -y qemu
choco install -y docker-desktop
choco install -y rustup.install

echo.
echo ====================================
echo Installation Complete!
echo ====================================
echo.
echo Next steps:
echo 1. Close this window and restart your terminal (Git Bash)
echo 2. Start Docker Desktop from the Start menu
echo 3. In Git Bash, run:
echo    rustup install nightly
echo    rustup default nightly
echo    rustup component add rust-src --toolchain nightly
echo 4. Verify with: ./verify-windows-setup.sh
echo 5. Build with: make run
echo.
echo See WINDOWS-SETUP.md for more details
echo.
pause
