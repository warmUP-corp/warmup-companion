# Warmup Companion

Windows companion service for warmUP gamepad input and the native gamepad-driven
virtual keyboard. It lets a controller open and drive a keyboard in normal apps,
UAC, lock, and sign-in surfaces where the desktop webview cannot inject input.

## Trust Model

This repo is intended to be auditable before install.

- The keyboard injects keys through Win32 `SendInput`.
- It does not read text from the focused application to power suggestions.
- Prediction context is a VK-only buffer: only characters typed by this keyboard
  are used.
- Text prediction is local, English-only prefix completion in userland.
- Predictions are disabled on UAC, lock, and sign-in surfaces.
- A local personal dictionary may be stored under `%LOCALAPPDATA%`, but writes
  are skipped when UI Automation reports the focused field is a password field.
- If password-field detection fails, learning is skipped.
- Crash telemetry is off unless `WARMUP_SENTRY_DSN` is explicitly set.

The service needs high Windows privileges because secure desktop input is not
available to a normal Tauri/webview process. The installed service is
`WarmupVkSvc`, runs as LocalSystem, and launches a worker into the active console
session.

## Install

From an Administrator PowerShell:

```powershell
.\install\Install-WarmupVk.ps1
```

The installer builds the release binary, installs:

- service: `WarmupVkSvc`
- binary: `C:\ProgramData\WarmupVk\bin\warmup-companion.exe`
- log: `C:\ProgramData\WarmupVk\service.log`

Then lock Windows or return to the sign-in screen and press the configured
controller VK button.

## Uninstall

From an Administrator PowerShell:

```powershell
.\target\release\warmup-companion.exe uninstall
```

or use the tray menu action when the installed companion is running.

## Diagnostics

```powershell
.\install\Collect-WarmupVkDiagnostics.ps1
```

This prints service status, installed binary metadata, recent service logs, and
HID/Winlogon signal lines. Crash dumps, when created, are local files under
`C:\ProgramData\WarmupVk`; do not share them without reviewing contents.

## Standalone Game Sleep

When warmUP is connected, it pushes `gameActive` / `launcherForegroundNav` over
IPC and the companion sleeps the controller loop while the game owns the pad.
Standalone companion builds also detect a foreground fullscreen game-like window
locally using the same warmUP-style fullscreen/window denylist heuristic.
While warmUP is connected over IPC, warmUP owns the mode state and standalone
detection is ignored.

Default: enabled.

```powershell
warmup-companion.exe settings sleep-on-game get
warmup-companion.exe settings sleep-on-game off
warmup-companion.exe settings sleep-on-game on
warmup-companion.exe settings auto-stop-on-game get
warmup-companion.exe settings auto-stop-on-game on
```

Equivalent config key in `%LOCALAPPDATA%\WarmupVk\settings.ini`:

```ini
sleep_on_game=true
auto_stop_on_game=false
```

## Sentry

Sentry is opt-in:

```powershell
$env:WARMUP_SENTRY_DSN = "https://public-key@o0.ingest.sentry.io/project"
```

Optional:

```powershell
$env:WARMUP_SENTRY_ENV = "production"
$env:WARMUP_SENTRY_RELEASE = "warmup-companion@0.1.0"
$env:WARMUP_SENTRY_DISABLED = "1"
```

The integration disables default PII, server name, tracing, logs, and metrics.
Native SEH crashes send a fatal summary only; local minidumps are not uploaded.

## Build

```powershell
cargo build --release
cargo check
```

Feature defaults include the Windows service and gamepad support.

## Architecture Notes

- [Domain glossary](CONTEXT.md)
- [Local prediction ADR](docs/adr/0001-local-prediction-sendinput.md)
- [Companion input authority ADR](docs/adr/0002-companion-gamepad-input-authority.md)
- [IPC protocol](docs/companion-ipc-protocol.md)
