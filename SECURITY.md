# Security

## Reporting

Report security issues privately to the warmUP maintainers before public
disclosure. Include affected version, reproduction steps, and expected impact.

## Threat Model

Warmup Companion runs with elevated Windows capability because it must support
secure desktop input. High-risk areas:

- LocalSystem service install and worker launch.
- Winlogon/UAC desktop attach.
- `SendInput` injection.
- UI Automation password-field detection.
- Named pipe IPC with the warmUP desktop.
- Native keyboard suppression and service/registry restoration.
- Crash dumps and optional telemetry.

## Safe Defaults

- Sentry is disabled unless `WARMUP_SENTRY_DSN` is set.
- Prediction does not read host application text.
- Prediction is disabled on secure desktop surfaces.
- Personal-dictionary learning skips password fields and UIA failures.
- IPC pipe is local and ACL'd to the interactive user.

## Local Artifacts

- Service log: `C:\ProgramData\WarmupVk\service.log`
- Service binary: `C:\ProgramData\WarmupVk\bin\warmup-companion.exe`
- Crash dumps: `C:\ProgramData\WarmupVk\worker-crash-*.dmp`
- User settings: `%LOCALAPPDATA%\WarmupVk\settings.ini`
- Personal dictionary: `%LOCALAPPDATA%\WarmupKeyboard\personal.dict`
