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
  Install log (silent runs): C:\ProgramData\WarmupVk\install.log

  Runs silently with no PowerShell window when started in its own process
  (double-clicked / "Run with PowerShell", launched with -File/-Command, or via
  the elevated relaunch below). In that case output is captured to install.log
  instead. When run from an interactive PowerShell session, output is shown in
  that session as usual and no window is hidden.

  C:\Program Files\WarmupVk\ is NOT used (legacy manual copies only).
#>
param(
    [Alias("Debug")]
    [switch]$DebugUi,
    # Optional offline voice typing: download a speech engine so the on-screen Mic
    # key appears. Omitted = no download, Mic key stays hidden. "parakeet" picks the
    # NVIDIA Parakeet engine instead of whisper (and builds with --features parakeet).
    [switch]$Speech,
    [ValidateSet("tiny", "base", "small", "medium", "parakeet")]
    [string]$SpeechModel = "medium"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

# Silent installer: show no PowerShell window.
#
# When this script runs in its own PowerShell process (the elevated relaunch
# below, a "Run with PowerShell" double-click, or any launcher that starts it
# with -File/-Command) we hide the console window so nothing flashes on screen.
# We deliberately skip this when the script is run inside an interactive
# PowerShell session, so we never hide a terminal the user is actively using.
# In silent runs, output is captured to install.log instead (see below).
$script:WarmupSilent = $false
function Hide-WarmupConsoleWindow {
    $launchedAsScript = $false
    foreach ($a in [Environment]::GetCommandLineArgs()) {
        if ($a -ieq "-File" -or $a -ieq "-Command" -or $a -ieq "-c" -or
            $a -ieq "-EncodedCommand" -or $a -ieq "-e" -or $a -ieq "-ec") {
            $launchedAsScript = $true
            break
        }
    }
    if (-not $launchedAsScript) { return }
    $script:WarmupSilent = $true

    if (-not ("WarmupVk.NativeWindow" -as [type])) {
        try {
            Add-Type -Namespace WarmupVk -Name NativeWindow -MemberDefinition @'
[DllImport("kernel32.dll")] public static extern System.IntPtr GetConsoleWindow();
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr hWnd, int nCmdShow);
'@
        } catch { return }
    }
    try {
        $hwnd = [WarmupVk.NativeWindow]::GetConsoleWindow()
        if ($hwnd -ne [IntPtr]::Zero) {
            [void][WarmupVk.NativeWindow]::ShowWindow($hwnd, 0)  # 0 = SW_HIDE
        }
    } catch { }
}
Hide-WarmupConsoleWindow

function Test-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]$identity
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Admin)) {
    $args = @(
        "-NoProfile",
        "-WindowStyle",
        "Hidden",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        "`"$PSCommandPath`""
    )
    if ($DebugUi) {
        $args += "-DebugUi"
    }
    if ($Speech) {
        $args += "-Speech"
        $args += @("-SpeechModel", $SpeechModel)
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

# Running silently (no window): capture the disclosure and status output to a
# log so the install can still be reviewed. The transcript ends when this
# dedicated installer process exits.
if ($script:WarmupSilent) {
    $WarmupInstallLogDir = "C:\ProgramData\WarmupVk"
    try {
        if (-not (Test-Path $WarmupInstallLogDir)) {
            New-Item -ItemType Directory -Path $WarmupInstallLogDir -Force | Out-Null
        }
        Start-Transcript -Path (Join-Path $WarmupInstallLogDir "install.log") -Append | Out-Null
    } catch { }
}

$BinDir = "C:\ProgramData\WarmupVk\bin"
$BinExe = Join-Path $BinDir "warmup-companion.exe"
$IconSrc = if ($env:WARMUP_ICON_PATH) { $env:WARMUP_ICON_PATH } else { Join-Path $Root "assets\icon.ico" }
$IconDest = Join-Path $BinDir "icon.ico"
$DataDir = "C:\ProgramData\WarmupVk"
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

Write-Host "=== Warmup Companion install disclosure ===" -ForegroundColor Cyan
Write-Host "Service:       WarmupVkSvc (LocalSystem, auto-start)"
Write-Host "Binary path:   $BinExe"
Write-Host "Data/log path: C:\ProgramData\WarmupVk"
Write-Host "Reason:        secure desktop input for sign-in, lock, and UAC"
Write-Host "Telemetry:     disabled unless WARMUP_SENTRY_DSN is set"
Write-Host "Privacy:       no host-control text reads for prediction; VK-only local context"
Write-Host "Game sleep:    enabled by default; fullscreen game detection sleeps poll to Guide-only"
Write-Host "Recovery:      tray menu and CLI command 'restore-keyboard' restore Windows keyboard services"
Write-Host "Voice typing:  $(if ($Speech) { "downloading '$SpeechModel' (offline, opt-in)" } else { 'skipped — Mic key hidden (pass -Speech to enable)' })"
Write-Host ""

# Parakeet's ASR code is behind a cargo feature (keeps non-parakeet builds lean);
# pull it in only when that engine was chosen.
$FeatureArgs = @()
if ($Speech -and $SpeechModel -eq "parakeet") { $FeatureArgs = @("--features", "parakeet") }
Write-Host "Building release (service + gamepad$(if ($FeatureArgs) { ' + parakeet' }))..."
cargo build --release @FeatureArgs
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

# Optional offline voice typing. Non-fatal: a download failure must not undo the
# service install — the companion runs fine without it (Mic key just stays hidden).
if ($Speech) {
    Write-Host "Installing offline voice typing ('$SpeechModel')..."
    try {
        if ($SpeechModel -eq "parakeet") {
            & (Join-Path $PSScriptRoot "Get-WarmupParakeet.ps1")
        } else {
            & (Join-Path $PSScriptRoot "Get-WarmupSpeech.ps1") -Model $SpeechModel
        }
    } catch {
        Write-Host "WARNING: voice-typing download failed: $($_.Exception.Message)" -ForegroundColor Yellow
        Write-Host "         The companion works without it; re-run with -Speech later, or drop" -ForegroundColor Yellow
        Write-Host "         whisper-server.exe + a ggml-*.bin into C:\ProgramData\WarmupVk\speech." -ForegroundColor Yellow
    }
}

if (Test-Path $IconSrc) {
    Copy-Item -LiteralPath $IconSrc -Destination $IconDest -Force
    Write-Host "OK: tray icon installed at $IconDest"
} else {
    Write-Host "WARNING: tray icon source missing: $IconSrc" -ForegroundColor Yellow
}

foreach ($Doc in @("README.md", "PRIVACY.md", "SECURITY.md", "LICENSE")) {
    $SrcDoc = Join-Path $Root $Doc
    if (Test-Path $SrcDoc) {
        Copy-Item -LiteralPath $SrcDoc -Destination (Join-Path $DataDir $Doc) -Force
    }
}

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
Write-Host "Tray icon:      $IconDest"
Write-Host "Log file:       $LogFile"
Write-Host "Trust docs:     C:\ProgramData\WarmupVk\README.md / PRIVACY.md / SECURITY.md"
Write-Host "Debug UI:       $(if ($DebugUi) { 'enabled' } else { 'disabled' })"
Write-Host "Sentry:         $(if ($env:WARMUP_SENTRY_DSN) { 'enabled by WARMUP_SENTRY_DSN' } else { 'disabled' })"
Write-Host ""
Write-Host "Important trust notes:"
Write-Host "  - The VK uses SendInput; it does not read host app text for prediction."
Write-Host "  - Prediction is local and disabled on UAC, lock, and sign-in surfaces."
Write-Host "  - Secure-desktop use may temporarily suppress Windows touch keyboard surfaces."
Write-Host "  - Restore command: $BinExe restore-keyboard"
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
