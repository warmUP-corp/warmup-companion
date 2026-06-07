# Privacy

Warmup Companion is designed for local input, not text collection.

## What The VK Does

- Sends keyboard input with Win32 `SendInput`.
- Tracks only text entered through the VK for prediction ranking.
- Stores optional local personal dictionary entries under `%LOCALAPPDATA%`.
- Uses UI Automation on the focused element only to decide whether dictionary
  learning is safe for password fields.

## What The VK Does Not Do

- It does not read text from arbitrary focused controls for prediction context.
- It does not upload prediction context or personal dictionary words.
- It does not enable prediction on UAC, lock, or sign-in surfaces.
- It does not send telemetry unless Sentry is configured with
  `WARMUP_SENTRY_DSN`.

## Password Fields

When a word is completed, the companion checks whether the focused element is a
password field. If it is, or if detection fails, the word is not written to the
personal dictionary. Typing still works.

## Windows Touch Keyboard Suppression

On secure desktop surfaces, Windows may summon its own touch keyboard. Warmup
temporarily suppresses those windows so the controller keyboard remains usable.
This can include:

- TabletTip auto-invoke registry values for the current user and `.DEFAULT`.
- `TabletInputService` start value, with prior value saved in process.
- Live stop/start of `TextInputManagementService`.
- Closing or terminating `TextInputHost.exe`, `TabTip.exe`, or `osk.exe` only on
  Winlogon when they conflict with PIN/password entry.

The tray and CLI expose restore paths so users can recover Windows input
services without reinstalling.

## Crash Telemetry

Sentry is disabled by default. If enabled:

- default PII is disabled
- server name is disabled
- tracing/log/metric capture is disabled
- Rust panics include stack traces
- native SEH crashes send only a fatal summary
- minidump files remain local

Crash dumps in `C:\ProgramData\WarmupVk` can contain sensitive memory. Review
before sharing.
