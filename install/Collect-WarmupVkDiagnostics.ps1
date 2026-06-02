<#
.SYNOPSIS
  Collect WarmupVk service diagnostics after a Winlogon gamepad test.

.DESCRIPTION
  Reads the installed service state and C:\ProgramData\WarmupVk\service.log,
  then prints the lines that prove whether DualShock / DualSense raw HID reports
  reached the secure-desktop helper.
#>
param(
    [int]$Tail = 240,
    [switch]$All
)

$ErrorActionPreference = "Stop"

$ServiceName = "WarmupVkSvc"
$DataDir = "C:\ProgramData\WarmupVk"
$BinExe = Join-Path $DataDir "bin\warmup-companion.exe"
$LogFile = Join-Path $DataDir "service.log"

Write-Host "=== WarmupVk Service ==="
sc.exe qc $ServiceName
sc.exe query $ServiceName

Write-Host ""
Write-Host "=== Installed Binary ==="
if (Test-Path $BinExe) {
    $item = Get-Item $BinExe
    Write-Host "Path:      $BinExe"
    Write-Host "Modified:  $($item.LastWriteTime)"
    Write-Host "Size:      $($item.Length)"
} else {
    Write-Host "Missing:   $BinExe"
}

Write-Host ""
Write-Host "=== Recent Service Log ==="
if (-not (Test-Path $LogFile)) {
    Write-Host "Missing: $LogFile"
    exit 1
}

$logLines = Get-Content $LogFile
$installMarker = "installed from "
$lastInstall = -1
for ($i = 0; $i -lt $logLines.Count; $i++) {
    if ($logLines[$i].Contains($installMarker)) {
        $lastInstall = $i
    }
}

$scopedLines = $logLines
if ((-not $All) -and $lastInstall -ge 0) {
    $scopedLines = $logLines[$lastInstall..($logLines.Count - 1)]
    Write-Host "(showing lines since latest install marker; pass -All for full log)"
}

$scopedLines | Select-Object -Last $Tail

Write-Host ""
Write-Host "=== HID / Winlogon Signal ==="
$patterns = @(
    "HID secure:",
    "hid:btn=0x",
    "HID slot 0",
    "raw input sink registered",
    "XInput secure helper",
    "input desktop",
    "Winlogon",
    "GUIDE"
)

$matches = $scopedLines |
    Where-Object {
        $line = $_
        $patterns | Where-Object { $line.Contains($_) } | Select-Object -First 1
    } |
    Select-Object -Last $Tail

if ($matches) {
    $matches
} else {
    Write-Host "No HID/Winlogon diagnostic lines found in $LogFile"
}

Write-Host ""
Write-Host "Expected physical-test signal:"
Write-Host "  hid:btn=0x.... changes when DualShock buttons/PS button are pressed"
Write-Host "  lt/rt change when L2/R2 are pressed"
Write-Host "  L(...,...) / R(...,...) changes when sticks move"
