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
- Installer runs silently (no PowerShell window) when started in its own
  process and writes its output to `C:\ProgramData\WarmupVk\install.log`.
- Release notes list service name, install path, log path, uninstall command,
  silent-install behavior and install log path, Sentry opt-in behavior, and
  native keyboard suppression/restoration behavior.
- Crash-dump handling is documented.

## Installer

- Build release assets and checksum sidecars:

```powershell
.\tools\New-ReleaseArtifacts.ps1
```

- Output: `target\release-assets\warmup-companion.exe`,
  `target\release-assets\warmup-companion.exe.sha256`,
  `target\release-assets\warmup-companion-setup.exe`, and
  `target\release-assets\warmup-companion-setup.exe.sha256`.
- The bare exe checksum sidecar must be attached to GitHub releases as exactly
  `warmup-companion.exe.sha256`; the desktop app refuses in-app companion
  installs when this asset is missing or mismatched.
- The installer is NSIS, UAC-elevated, and supports silent `/S`.
- Default release builds include the Parakeet engine code. The installer still
  downloads the large offline speech model only when voice typing is selected.
- Attach both `warmup-companion-setup.exe` and the bare `warmup-companion.exe`
  to the release, each with a SHA-256 checksum.
- Sign both before publishing (see Binary Signing).

## Binary Signing

- Sign `warmup-companion.exe` before publishing.
- Sign `target\warmup-companion-setup.exe` (the NSIS installer) before
  publishing.
- Sign `install\Install-WarmupVk.ps1` with the same certificate
  (`Set-AuthenticodeSignature`); the installer is part of the trust story.
- Include certificate subject and thumbprint in release notes.
- Publish SHA-256 checksums for release assets.

Download-verification command published in the release notes:

```powershell
(Get-FileHash .\warmup-companion.exe -Algorithm SHA256).Hash.ToLower()
```

### Getting a certificate (open source)

Since June 2023, code-signing keys (OV and EV) must live on FIPS-140 hardware
or a cloud HSM; downloadable `.pfx` files are no longer issued. Choose a key
custodian first, then sign in CI.

- **SignPath Foundation** — recommended; free for qualifying open-source
  projects. Sponsored OV certificate, key held in SignPath's cloud HSM, signing
  wired into GitHub Actions. Requires an OSI-approved license and a minimum
  project-activity bar.
- **Certum Open Source** — low-cost OV certificate issued to an individual after
  ID verification; key on Certum SimplySign cloud or a USB card.
- **Azure Trusted Signing** — low-cost Microsoft-run signing with HSM included
  and first-class GitHub Actions support; individual eligibility needs a
  verifiable multi-year identity history.

### SmartScreen reputation

- An OV signature removes the "Unknown publisher" label, but SmartScreen
  reputation accrues over downloads; early downloads may still warn.
- Only an EV certificate grants instant SmartScreen reputation, and EV is not
  available for free.
- MSIX / Microsoft Store signing is not viable here: the app installs a
  LocalSystem service for secure-desktop input, which does not fit the Store
  sandbox.

### Until a certificate is in place

- Ship unsigned with a published SHA-256 checksum and document the SmartScreen
  "More info -> Run anyway" step. This is a documented stopgap, not the final
  state.

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
