# Kevlar - Windows Build Setup

This guide explains how to build and run Kevlar on Windows using Git Bash, uv, and native Windows tools.

## Prerequisites

You'll need:
- Git Bash (MINGW64) - typically installed with Git for Windows
- [uv](https://docs.astral.sh/uv/) - Modern Python package manager (recommended)
- Chocolatey package manager (for installing build tools)

## Quick Start with uv

If you already have `uv` installed (check with `uv --version`), you can use it for all Python dependencies:

```bash
# uv will automatically manage Python and dependencies
# No need to install Python separately!
```

## Installing uv (if not installed)

```bash
# Windows (PowerShell)
powershell -c "irm https://astral.sh/uv/install.ps1 | iex"

# Or via pip (if you have Python)
pip install uv
```

## Installing Chocolatey

If you don't have Chocolatey installed, open PowerShell as Administrator and run:

```powershell
Set-ExecutionPolicy Bypass -Scope Process -Force; [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072; iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
```

## Installing Build Dependencies

Open PowerShell as Administrator and run:

```powershell
# Install Visual Studio Build Tools (required for Rust MSVC linker)
choco install -y visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"

# Install build tools
choco install -y make

# Install QEMU
choco install -y qemu

# Install Docker Desktop
choco install -y docker-desktop

# Install Rust
choco install -y rustup.install
```

After installation:

1. **Restart your terminal** (Git Bash or PowerShell) to reload PATH
2. **Start Docker Desktop** from the Start menu
3. **Configure Docker**: Make sure Docker is running in Linux containers mode (not Windows containers)

**Note**: Visual Studio Build Tools installation may take 10-15 minutes and requires ~6GB of disk space. This provides the MSVC linker (`link.exe`) that Rust needs to compile build scripts.

## Installing Rust Nightly

After rustup is installed, open Git Bash and run:

```bash
# Add cargo to PATH for this session
export PATH="$HOME/.cargo/bin:$PATH"

# Install Rust nightly
rustup install nightly
rustup default nightly
rustup component add rust-src --toolchain nightly

# Install rustfilt (needed for demangling Rust symbols)
cargo install rustfilt
```

**Important**: Add `%USERPROFILE%\.cargo\bin` to your system PATH permanently so cargo tools are always available:
1. Press Win+R, type `sysdm.cpl`, press Enter
2. Go to "Advanced" tab → "Environment Variables"
3. Under "User variables", select "Path" → "Edit"
4. Click "New" and add: `%USERPROFILE%\.cargo\bin`
5. Click OK on all dialogs
6. Restart your terminal

## Verifying Installation

Run the verification script in Git Bash:

```bash
chmod +x verify-windows-setup.sh
./verify-windows-setup.sh
```

Or manually verify:

```bash
make --version          # Should show GNU Make
uv --version            # Should show uv version
rustc --version         # Should show nightly version
cargo --version         # Should show cargo version
qemu-system-x86_64 --version    # Should show QEMU version
docker --version        # Should show Docker version
```

## Building Kevlar

Once all dependencies are installed:

```bash
# Build and run on x86_64 (make detects tools automatically on Windows)
make run

# Build only (without running)
make build

# Alternative: Use the wrapper script (always sets PATH)
./make-windows.sh run

# Build and run on ARM64 (release mode recommended for performance)
RELEASE=1 ARCH=arm64 make run
```

**Note**: The Makefile automatically detects cargo and Docker in standard locations on Windows. The `make-windows.sh` wrapper script is provided for convenience but is no longer required.

## Troubleshooting

### MSVC linker (link.exe) not found
If Rust complains about `link.exe` not found, you need Visual Studio Build Tools:

```powershell
# In PowerShell as Administrator
choco install -y visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"
```

After installation, restart your terminal. The linker should be automatically added to PATH by the Visual Studio installer.

### Python not found
If `python3` is not found but `python` works, the Makefile will use `python` instead (already configured).

### Docker daemon not running
Make sure Docker Desktop is running. You should see the Docker whale icon in the system tray.

### QEMU not in PATH
After installing QEMU via Chocolatey, you may need to:
1. Restart your terminal
2. Or manually add `C:\Program Files\qemu` to your system PATH

### Make not found
After installing via Chocolatey, restart your Git Bash terminal. Make should be at `C:\ProgramData\chocolatey\bin\make.exe`.

### Port conflicts when running QEMU
The run script tries to use ports 20022 and 20080. If these are in use:
- Close any running QEMU instances
- Or use `QEMU_ARGS` to override port forwarding

## Windows-Specific Notes

1. **Docker must use Linux containers** - Kevlar builds Linux kernel binaries, so Docker must be in Linux container mode
2. **Git Bash recommended** - The build system uses Unix-style shell commands, so Git Bash (MINGW64) provides the best compatibility
3. **Line endings** - Git should be configured to use LF line endings. If you see issues, run:
   ```bash
   git config --global core.autocrlf input
   ```

## Quick Start Script

For a fully automated setup (run in PowerShell as Administrator):

```powershell
.\setup-windows.ps1
```

This will install all dependencies and verify the installation.
