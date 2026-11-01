<#
.SYNOPSIS
  Installs Vox for the current user on Windows (no administrator rights needed).

.DESCRIPTION
  - Uses prebuilt vox.exe / vox-tray.exe from -BinDir, or from target\release
    (building with `cargo build --release` first when Cargo is available).
  - Copies them to %LOCALAPPDATA%\Programs\Vox and puts that on your PATH.
  - Adds a "Vox" Start-menu shortcut, downloads the speech model, enables
    launch-at-login, and starts the tray.

  Re-running it upgrades in place; settings and history are kept.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1

.EXAMPLE
  .\scripts\install-windows.ps1 -Model small-q5_1 -NoAutostart
#>
[CmdletBinding()]
param(
    [string]$BinDir,
    [string]$Model = "",
    [switch]$NoAutostart,
    [switch]$NoBuild,
    [switch]$NoStart,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Vox"
$StartMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"

function Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Run($desc, [scriptblock]$action) {
    if ($DryRun) { Write-Host "    [dry-run] $desc" } else { & $action }
}

# 1. Binaries
if (-not $BinDir) {
    $BinDir = Join-Path $Root "target\release"
    $haveBins = (Test-Path (Join-Path $BinDir "vox.exe")) -and (Test-Path (Join-Path $BinDir "vox-tray.exe"))
    if (-not $NoBuild -and (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Step "Building Vox (cargo build --release)"
        Run "cargo build --release --locked" {
            Push-Location $Root
            try {
                cargo build --release --locked
                if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
            } finally { Pop-Location }
        }
    } elseif (-not $haveBins) {
        throw "No Vox binaries found in $BinDir. Install Rust (https://rustup.rs) and re-run, or pass -BinDir pointing at a release download."
    }
}
foreach ($exe in @("vox.exe", "vox-tray.exe")) {
    if (-not $DryRun -and -not (Test-Path (Join-Path $BinDir $exe))) {
        throw "missing $exe in $BinDir"
    }
}

# 2. Stop a running tray so its exe can be replaced
Step "Stopping any running Vox tray"
Run "Stop-Process vox-tray, vox" {
    Get-Process -Name "vox-tray" -ErrorAction SilentlyContinue | Stop-Process -Force
    Get-CimInstance Win32_Process -Filter "Name = 'vox.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -match '\btray\b' } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
    Start-Sleep -Milliseconds 300
}

# 3. Copy
Step "Installing to $InstallDir"
Run "copy binaries" {
    New-Item -ItemType Directory -Force $InstallDir | Out-Null
    Copy-Item (Join-Path $BinDir "vox.exe") $InstallDir -Force
    Copy-Item (Join-Path $BinDir "vox-tray.exe") $InstallDir -Force
}
$VoxExe = Join-Path $InstallDir "vox.exe"
$TrayExe = Join-Path $InstallDir "vox-tray.exe"

# 4. PATH (user scope)
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $userPath) { $userPath = "" }
if (($userPath -split ";") -notcontains $InstallDir) {
    Step "Adding $InstallDir to your PATH"
    Run "update user PATH" {
        $new = if ($userPath.Trim()) { "$($userPath.TrimEnd(';'));$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $new, "User")
    }
}

# 5. Start-menu shortcut
Step "Creating Start-menu shortcut"
Run "create Vox.lnk" {
    $shell = New-Object -ComObject WScript.Shell
    $lnk = $shell.CreateShortcut((Join-Path $StartMenu "Vox.lnk"))
    $lnk.TargetPath = $TrayExe
    $lnk.WorkingDirectory = $InstallDir
    $lnk.Description = "Vox - local voice dictation (Ctrl+.)"
    $lnk.Save()
}

# 6. Speech model
Step "Downloading the speech model (one time)"
Run "vox download-model $Model" {
    if ($Model) { & $VoxExe download-model $Model } else { & $VoxExe download-model }
    if ($LASTEXITCODE -ne 0) { throw "model download failed" }
}

# 7. Launch at login
if (-not $NoAutostart) {
    Step "Enabling launch at login"
    Run "vox autostart on" { & $VoxExe autostart on | Out-Null }
}

# 8. Start
if (-not $NoStart) {
    Step "Starting Vox"
    Run "start vox-tray" { Start-Process -FilePath $TrayExe -WorkingDirectory $InstallDir }
}

Write-Host ""
Write-Host "Vox is installed." -ForegroundColor Green
Write-Host "  Click into any text field and press Ctrl+. to dictate:"
Write-Host "    tap Ctrl+. to start, tap again to finish  (or hold it while you talk)"
Write-Host "    Esc cancels"
Write-Host "  Settings: right-click the Vox tray icon > Settings  (http://127.0.0.1:8722)"
Write-Host "  Health check: vox doctor   (open a new terminal so PATH updates apply)"
