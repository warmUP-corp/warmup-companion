#Requires -RunAsAdministrator
<#
.SYNOPSIS
  Build and install WarmupVkSvc (sign-in / UAC gamepad keyboard).

.DESCRIPTION
  Copies release exe to C:\ProgramData\WarmupVk\bin, registers Windows service
  WarmupVkSvc with --service (--boot + winlogon). After install, reboot or Win+L
  and tap Y on the controller at the password screen.

  Log: C:\ProgramData\WarmupVk\service.log
#>
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

Write-Host "Building release (gamepad + service)..."
cargo build --release --features service
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Exe = Join-Path $Root "target\release\warmup-vk-prototype.exe"
if (-not (Test-Path $Exe)) {
    throw "Missing $Exe"
}

Write-Host "Installing service (also removes WarmupVkTest* debug services)..."
& $Exe install
exit $LASTEXITCODE
