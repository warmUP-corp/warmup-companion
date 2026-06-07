# Release Checklist

Use this before publishing an OSS release or binary.

## Build Provenance

- CI green on Windows.
- `cargo test --all-features` passes.
- Release commit is tagged.
- `Cargo.lock` is committed.
- Binary is built from the tagged commit.

## Trust Artifacts

- `README.md`, `PRIVACY.md`, `SECURITY.md`, `LICENSE`, and `CONTRIBUTING.md`
  are present in the repo.
- Installer copies trust docs to `C:\ProgramData\WarmupVk`.
- Release notes list service name, install path, log path, uninstall command,
  Sentry opt-in behavior, and native keyboard suppression/restoration behavior.
- Crash-dump handling is documented.

## Binary Signing

- Sign `warmup-companion.exe` before publishing.
- Include certificate subject and thumbprint in release notes.
- Publish SHA-256 checksums for release assets.

## Privacy / Telemetry

- Confirm Sentry is disabled without `WARMUP_SENTRY_DSN`.
- Confirm no DSN is compiled into public artifacts.
- Confirm `send_default_pii=false`, no server name, no tracing, no log capture,
  and no metric capture.

## Recovery

- Verify tray actions:
  - open service log
  - run diagnostics
  - show privacy summary
  - restore Windows keyboard services
  - uninstall
- Verify CLI recovery:

```powershell
warmup-companion.exe restore-keyboard
warmup-companion.exe uninstall
```
