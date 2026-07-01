# Companion IPC protocol

**Status:** v4. Companion (`warmup-keyboard`) ⇄ warmUP desktop.

This spec is the **versioned wire contract** between the two processes (ADR `0002`). It is mirrored verbatim in both repos. Changing any frame shape or the pipe framing is a breaking change and **must** bump `protocolVersion`.

## Transport

- **Pipe:** named pipe `\\.\pipe\warmup-input`.
- **Roles:** companion = **server**, desktop = **reconnecting client**. Both auto-start in indeterminate order; the client retries with backoff until the server is up, and reconnects on server restart.
- **ACL:** the pipe is ACL'd to the **interactive user** only (the active console session the companion is launched into). No network exposure.
- **Direction:** full-duplex. "Up" = companion→desktop (input). "Down" = desktop→companion (control).

## Framing

**Newline-delimited JSON (NDJSON).** One JSON object per line, terminated by a single `\n` (`0x0A`). No embedded newlines inside a frame. UTF-8.

Every frame is **adjacently tagged**:

```json
{"type": "<frameType>", "payload": { ... }}
```

- `type` — snake_case frame name (catalog below).
- `payload` — object whose fields are **camelCase**, matching the desktop's existing webview event shapes 1:1 (so the desktop layer is a pass-through, no remap).

Serde representation: `#[serde(tag = "type", content = "payload", rename_all = "snake_case")]`.

## Handshake — `hello`

The client sends `hello` as its **first** frame after connecting. The server replies with its own `hello`. If `protocolVersion` differs, the **server closes the connection** (the client backs off and retries — typically after one side is upgraded).

```json
{"type":"hello","payload":{
  "protocolVersion": 4,
  "config": { ...GamepadConfig },
  "mode": {
    "gameActive": false,
    "launcherForegroundNav": false,
    "clicksEnabled": true,
    "launcherOwnsTextInput": false,
    "browserActive": false
  },
  "companionSettings": { "sleepOnGame": true, "autoStopOnGame": false }
}}
```

| Field | Who sends | Notes |
|---|---|---|
| `protocolVersion` | both | single integer; bumped on any breaking wire change |
| `config` | client | desktop's current `GamepadConfig` snapshot (so the companion starts with the right tuning); server omits |
| `mode` | client | desktop's current mode snapshot; server omits |
| `companionSettings` | client | desktop-pushed companion-local settings snapshot; server omits |

## Up frames (companion → desktop)

Each maps to an existing webview event; the desktop re-emits the `payload` unchanged.

| `type` | webview event | `payload` shape |
|---|---|---|
| `connection` | `gamepad:connection` | `{ connected: bool, controllerType: string, controllerName: string }` |
| `button` | `gamepad:button` | `{ button: string, pressed: bool, controllerType: string }` |
| `battery` | `gamepad:battery` | `{ percent: i32, charging: bool, wired: bool }` |
| `cursor_moved` | `gamepad:cursor_moved` | `{ dx: f64, dy: f64 }` |
| `axis` | `gamepad:axis` | `{ leftX: f32, leftY: f32, rightX: f32, rightY: f32 }` |
| `touchpad` | `gamepad:touchpad` | `{ fingers: [{ index: u8, down: bool, x: f32, y: f32, pressure: f32 }] }` |

Notes:
- `connection.controllerType` / `button.controllerType`: `"xbox" | "playstation" | "switch" | "generic"` (existing desktop vocabulary).
- `battery.percent`: `0–100`, or `-1` when the controller reports no level. `wired` = no internal battery.
- `cursor_moved` / `axis` / `touchpad` are throttled by the companion (≈100 ms) exactly as the desktop poll thread does today.

## Down frames (desktop → companion)

| `type` | `payload` shape | Purpose |
|---|---|---|
| `config` | full `GamepadConfig` (below) | push tuning on change (`set_gamepad_config`) |
| `mode` | `{ gameActive: bool, launcherForegroundNav: bool, clicksEnabled: bool, launcherOwnsTextInput: bool, browserActive: bool }` | game-active sleep branch + launcher-foreground nav forwarding (#351), desktop web-VK ownership for native-VK suppression (#353), plus standalone browser ownership for companion-local L3/R3 (native VK / voice) |
| `rumble` | `{ kind: "full", strong: f32, weak: f32, durationMs: u32 }` **or** `{ kind: "triggers", left: f32, right: f32, durationMs: u32 }` | one-shot force feedback (#352) |
| `led` | `{ r: u8, g: u8, b: u8 }` | immediate LED/lightbar write for controller test actions; config still owns persisted/effect state |
| `companion_settings` | `{ sleepOnGame?: bool, autoStopOnGame?: bool, userlandPollPaused?: bool, promptUserlandDebug?: bool }` | companion-local runtime/settings control |

### `GamepadConfig` payload

camelCase, matching the desktop serde shape:

```json
{
  "deadzone": 0.15,
  "sensitivity": 15.0,
  "accelerationExp": 2.0,
  "scrollSensitivity": 5.0,
  "enabled": true,
  "launcherToggleButton": "GUIDE",
  "triggerRumbleEnabled": false,
  "triggerRumbleMagnitude": 0.5,
  "gyroScrollEnabled": false,
  "gyroScrollSensitivity": 1.0,
  "ledColor": "#b6a0ff",
  "ledSecondaryColor": "#4c7b99",
  "ledEffect": "gradient",
  "ledBrightness": 0.8,
  "vkMode": "docked",
  "keyboardTheme": {
    "background": "#1f1f1f",
    "key": "#2b2b2b",
    "accent": "#4c7b99",
    "text": "#ffffff",
    "selectedText": "#ffffff",
    "border": "#3a3a3a"
  }
}
```

`ledEffect` accepts `"solid"`, `"breathing"`, `"rainbow"`, `"gradient"`, or `"off"`.
`gradient` smoothly cycles between `ledColor` and `ledSecondaryColor`.
`ledBrightness` is a `0.0`-`1.0` multiplier applied to all companion-driven LED writes.

Added in v3: `keyboardTheme.border` (key outline, derived desktop-side from `key` to match the
webview VK) and `vkMode` (`"docked"` | `"floating"`; floating renders a compact panel near the
field instead of the full-width bottom dock). Both are additive and optional.

### `companion_settings` payload

camelCase; every field is optional so warmUP can send partial updates.

```json
{
  "sleepOnGame": true,
  "autoStopOnGame": false,
  "userlandPollPaused": false,
  "promptUserlandDebug": false
}
```

- `sleepOnGame`: persist companion standalone fullscreen-game sleep behavior.
- `autoStopOnGame`: persist companion standalone fullscreen-game auto-stop behavior.
- `userlandPollPaused`: runtime pause flag for userland polling; mirrored to runtime status.
- `promptUserlandDebug`: toggles the userland prompt debug overlay sentinel.

When warmUP is connected, warmUP mode frames are authoritative; standalone fullscreen detection
does not override `gameActive` / `launcherForegroundNav`.

The companion maps cursor/scroll tuning fields to its internal names per the golden fixture's `configFieldMapping` (`sensitivity->cursor_speed`, `accelerationExp->cursor_accel`, `deadzone->cursor_deadzone`, `scrollSensitivity->scroll_speed`).

`keyboardTheme` is optional, and each color inside it is optional. Colors are `#RRGGBB`;
the companion persists provided values as `keyboard_bg`, `keyboard_key`,
`keyboard_accent`, `keyboard_text`, and `keyboard_sel_text` in `settings.ini`. Missing
fields keep the native keyboard's current dark/light default for that color slot.

## Versioning policy

- `protocolVersion` is a **single integer**, currently `4`.
- `browserActive` is an additive v4-compatible mode field; old companions ignore it, new companions use it to keep browser L3/R3 local to native VK/voice.
- `axis` is an additive v4-compatible up-frame; old desktop clients ignore it as unknown, old companions simply omit it.
- `led` is an additive v4-compatible down-frame for one-shot test writes; old companions ignore it as unknown.
- Additive fields may stay on the current protocol when both directions are default/unknown-field tolerant.
- Breaking changes to the pipe name, framing, `hello` shape, or required frame `payload` fields bump it.
- A version mismatch is resolved by the server closing the connection; the client surfaces a "companion update required" state rather than interpreting unknown frames.

## Golden fixtures

Cursor/scroll math parity between the two independent implementations is guarded by a checked-in golden fixture (`(stickX, stickY, dt, config) -> (dx, dy)`). Format + loader are defined in #346 (`src/golden.rs` here, `apps/desktop/src-tauri/src/gamepad/golden.rs` in warmUP); parity assertions land in #349. See `tests/fixtures/cursor-scroll-golden.json`.
