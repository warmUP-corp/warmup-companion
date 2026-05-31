# 0002 — Companion is the Windows gamepad input authority

**Date:** 2026-05-31
**Status:** Accepted
**Context:** Companion-app epic (warmUP-corp/warmUP#346). This companion (`warmup-keyboard`, the `WarmupVkSvc` service worker) becomes the single Windows gamepad runtime, feeding the warmUP desktop over IPC. Mirror of warmUP `docs/adr/0003-companion-gamepad-input-authority.md`.

## Problem

The warmUP desktop currently owns gamepad input in-process via SDL3 + `enigo` injection. On Windows that is the wrong place:

- A sandboxed Tauri webview process cannot inject on the secure desktop / during elevated foreground apps.
- Two independent input owners (desktop SDL3 *and* this companion's XInput VK navigation) race for the same device.
- The native virtual keyboard already lives here, launched into the active console session by the SCM. Splitting input across two processes duplicates device logic and config.

## Decision

### 1. Service owns input (Windows)

`WarmupVkSvc` is the **single** Windows gamepad runtime: it owns the device (XInput on the secure desktop, SDL3 in `--gamepad` mode), performs all OS injection, and hosts the IPC pipe. The desktop is a pure **client** that consumes input frames and re-emits webview events; it does no device access or injection on Windows. The SCM owns this service's lifecycle.

### 2. Strict platform split

| Platform | Gamepad runtime | Native VK |
|---|---|---|
| Windows | companion (**mandatory**, no local fallback) | yes (Windows-only) |
| non-Windows | desktop in-process SDL3, unchanged | n/a |

No Windows local-fallback path. If the companion is not up, the desktop client reconnects with backoff (both auto-start in indeterminate order).

### 3. Versioned wire contract, not a shared crate

The boundary is a **versioned wire contract** (`docs/companion-ipc-protocol.md`), not a shared Rust crate — the two processes are built and deployed independently. Gamepad logic lives **only** here in the companion. The contract is compatible with the desktop's existing webview event shapes so the desktop adapter is a pass-through.

### 4. Math parity by golden fixtures, not shared code

Cursor/scroll math now lives here, but the desktop historically computed it. Both repos check in an identical golden-fixture file (`(stickX, stickY, dt, config) -> (dx, dy)`) plus the config field-mapping (`sensitivity->cursor_speed`, `accelerationExp->cursor_accel`, `deadzone->cursor_deadzone`, `scrollSensitivity->scroll_speed`). A loader exists in both repos now (`src/golden.rs`); parity assertions land with the injection slice (#349). This keeps the two independent implementations from drifting without a shared crate.

## Consequences

- The companion carries `sdl3` + `enigo`; the Windows desktop build sheds them (warmUP#347).
- New input capabilities (rumble, LED, battery, gyro, touchpad) are negotiated over the contract (warmUP#352).
- The contract is versioned (`protocolVersion`, single integer). A `hello` mismatch closes the connection rather than risk misreading a frame.
- No runtime behavior ships in #346 — decision/foundation slice only. The pipe server itself lands in warmUP#347.
