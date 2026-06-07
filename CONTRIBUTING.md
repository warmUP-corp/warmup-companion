# Contributing

## Before Changes

- Read `CONTEXT.md` for product terms.
- Read relevant ADRs in `docs/adr`.
- Keep trust-sensitive behavior documented when it changes.

## Development

```powershell
cargo check
cargo test
```

For secure desktop behavior, use the installer and diagnostics scripts:

```powershell
.\install\Install-WarmupVk.ps1
.\install\Collect-WarmupVkDiagnostics.ps1
```

## Review Focus

Changes touching service install, Winlogon, `SendInput`, UI Automation,
prediction, IPC, Sentry, registry, or crash dumps need explicit privacy/security
notes in the PR.
