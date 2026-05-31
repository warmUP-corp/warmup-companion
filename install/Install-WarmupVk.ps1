#Requires -RunAsAdministrator
<#
.SYNOPSIS
  Build and install Warmup Companion (sign-in / UAC gamepad keyboard).

.DESCRIPTION
  Same gamepad-enabled binary as:
    cargo build --release
  Desktop test (after install or from target\release):
    warmup-companion.exe --gamepad --real

  Service binary (SCM): C:\ProgramData\WarmupVk\bin\warmup-companion.exe
  Log: C:\ProgramData\WarmupVk\service.log

  C:\Program Files\WarmupVk\ is NOT used (legacy manual copies only).
#>
param(
    [switch]$DebugUi
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$BinDir = "C:\ProgramData\WarmupVk\bin"
$BinExe = Join-Path $BinDir "warmup-companion.exe"
$LogFile = "C:\ProgramData\WarmupVk\service.log"
$LegacyExe = "C:\Program Files\WarmupVk\warmup-vk-prototype.exe"

Write-Host "Building release (default service + gamepad features)..."
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Exe = Join-Path $Root "target\release\warmup-companion.exe"
if (-not (Test-Path $Exe)) {
    throw "Missing $Exe"
}

cmd /c "findstr /C:`"gamepad loop running`" /C:`"warmup-gamepad (SDL3)`" `"$Exe`" >nul 2>&1"
if ($LASTEXITCODE -ne 0) {
    throw "Built exe is missing gamepad support. Re-run this script."
}
Write-Host "OK: gamepad feature present in $Exe"

cmd /c "findstr /C:`"ds4-usb`" /C:`"ds5-usb`" /C:`"ds5-bt`" `"$Exe`" >nul 2>&1"
if ($LASTEXITCODE -ne 0) {
    throw "Built exe is missing PlayStation Winlogon HID support. Re-run this script."
}
Write-Host "OK: PlayStation Winlogon HID support present in $Exe"

Write-Host "Installing service..."
$InstallArgs = @("install")
if ($DebugUi) {
    $InstallArgs += "--debug-ui"
}
& $Exe @InstallArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if (-not (Test-Path $BinExe)) {
    throw @"
Install failed: service binary not at:
  $BinExe

The service is registered to use ProgramData, not Program Files.
If you only have $LegacyExe, delete it and run this script again as Administrator.
"@
}

Write-Host ""
Write-Host "=== Install OK ===" -ForegroundColor Green
Write-Host "Service binary: $BinExe"
Write-Host "Log file:       $LogFile"
Write-Host "Debug UI:       $(if ($DebugUi) { 'enabled' } else { 'disabled' })"
if (Test-Path $LegacyExe) {
    Write-Host "WARNING: legacy copy still exists (not used by service): $LegacyExe" -ForegroundColor Yellow
    Write-Host "         You can remove that folder to avoid confusion."
}

Write-Host ""
Write-Host "Service status:"
sc.exe qc WarmupVkSvc
sc.exe query WarmupVkSvc

if (Test-Path $LogFile) {
    Write-Host ""
    Write-Host "Last log lines:"
    Get-Content $LogFile -Tail 8
}

exit 0
