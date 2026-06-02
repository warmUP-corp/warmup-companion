<#
.SYNOPSIS
  Build and install Warmup Companion (sign-in / UAC gamepad keyboard).

.DESCRIPTION
  Same gamepad-enabled binary as:
    cargo build --release
  Desktop test (after install or from target\release):
    warmup-companion.exe --gamepad --real
  Install with debug overlays/hotkeys enabled:
    .\install\Install-WarmupVk.ps1 -Debug

  Service binary (SCM): C:\ProgramData\WarmupVk\bin\warmup-companion.exe
  Log: C:\ProgramData\WarmupVk\service.log

  C:\Program Files\WarmupVk\ is NOT used (legacy manual copies only).
#>
param(
    [Alias("Debug")]
    [switch]$DebugUi
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Test-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]$identity
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Admin)) {
    $args = @(
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        "`"$PSCommandPath`""
    )
    if ($DebugUi) {
        $args += "-DebugUi"
    }
    Write-Host "Administrator rights are required; opening an elevated installer..."
    try {
        Start-Process -FilePath "powershell.exe" -ArgumentList $args -Verb RunAs -WindowStyle Hidden -Wait
    } catch {
        throw "Elevated installer was not started. Approve the UAC prompt and try again. $($_.Exception.Message)"
    }
    Write-Host "Elevated installer finished. Run .\install\Collect-WarmupVkDiagnostics.ps1 to verify the installed service binary and logs."
    exit 0
}

$BinDir = "C:\ProgramData\WarmupVk\bin"
$BinExe = Join-Path $BinDir "warmup-companion.exe"
$LogFile = "C:\ProgramData\WarmupVk\service.log"
$LegacyExe = "C:\Program Files\WarmupVk\warmup-vk-prototype.exe"

function Test-BinaryString {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Needle
    )
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    return $text.Contains($Needle)
}

Write-Host "Building release (default service + gamepad features)..."
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Exe = Join-Path $Root "target\release\warmup-companion.exe"
if (-not (Test-Path $Exe)) {
    throw "Missing $Exe"
}

if (-not ((Test-BinaryString $Exe "gamepad loop running") -or (Test-BinaryString $Exe "warmup-gamepad (SDL3)"))) {
    throw "Built exe is missing gamepad support. Re-run this script."
}
Write-Host "OK: gamepad feature present in $Exe"

if (-not ((Test-BinaryString $Exe "ds4-usb") -or (Test-BinaryString $Exe "ds5-usb") -or (Test-BinaryString $Exe "ds5-bt"))) {
    throw "Built exe is missing PlayStation Winlogon HID support. Re-run this script."
}
Write-Host "OK: PlayStation Winlogon HID support present in $Exe"

if (-not ((Test-BinaryString $Exe "hid:btn=0x") -and (Test-BinaryString $Exe "GUIDE"))) {
    throw "Built exe is missing PlayStation Winlogon HID diagnostics/Guide support. Re-run this script."
}
Write-Host "OK: PlayStation Winlogon diagnostics + Guide support present in $Exe"

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
Write-Host ""
Write-Host "Winlogon DualShock test:"
Write-Host "  1. Lock Windows or switch to the sign-in screen."
Write-Host "  2. Press/move the DualShock, including PS, L2/R2, and sticks."
Write-Host "  3. Back on the desktop, run:"
Write-Host "     .\install\Collect-WarmupVkDiagnostics.ps1"
Write-Host "  Expected log signal: hid:btn=0x...., lt/rt, and L/R axes change."
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
