<#
.SYNOPSIS
  Removes Vox for the current user. Settings, history, and models in
  %LOCALAPPDATA%\Vox are kept unless -RemoveData is given.
#>
[CmdletBinding()]
param([switch]$RemoveData)

$ErrorActionPreference = "Stop"
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Vox"
$DataDir = Join-Path $env:LOCALAPPDATA "Vox"

Get-Process -Name "vox-tray" -ErrorAction SilentlyContinue | Stop-Process -Force
Get-CimInstance Win32_Process -Filter "Name = 'vox.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -match '\btray\b' } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force }

Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "Vox" -ErrorAction SilentlyContinue
Remove-Item (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Vox.lnk") -ErrorAction SilentlyContinue

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath) {
    $kept = ($userPath -split ";") | Where-Object { $_ -and ($_ -ne $InstallDir) }
    [Environment]::SetEnvironmentVariable("Path", ($kept -join ";"), "User")
}

if (Test-Path $InstallDir) { Remove-Item $InstallDir -Recurse -Force }
if ($RemoveData -and (Test-Path $DataDir)) { Remove-Item $DataDir -Recurse -Force }

Write-Host "Vox removed." -ForegroundColor Green
if (-not $RemoveData) { Write-Host "Your settings, history, and models are still in $DataDir (use -RemoveData to delete them)." }
