# Companion IPC protocol

**Status:** v6. Companion (`warmup-keyboard`) ⇄ warmUP desktop.

This spec is the **versioned wire contract** between the two processes. It is mirrored verbatim in both repos. Changing any frame shape or the pipe framing is a breaking change and **must** bump `protocolVersion`.

## Transport

- **Pipe:** named pipe `\\.\pipe\warmup-input`.
- **Roles:** companion = **server**, desktop = **reconnecting client**. Both auto-start in indeterminate order; the client retries with backoff until the server is up, and reconnects on server restart.
- **ACL:** the pipe is ACL'd to the **interactive user** only (the active console session the companion is launched into). No network exposure.
- **Development builds:** executable-path client trust checks are compiled out. Release builds only accept the installed warmUP executable.
- **Direction:** full-duplex. "Up" = companion→desktop (input). "Down" = desktop→companion (control).

### Transport (Linux)

Frames, the handshake, and the roles are the same as the named pipe. Only the socket differs.

- **Socket:** Unix domain socket `$XDG_RUNTIME_DIR/warmup/input.sock`. The companion creates the directory mode `0700` and the socket mode `0600`, and removes a stale socket before binding.
- **Peer check:** a connection is accepted only when `SO_PEERCRED` reports a uid equal to the companion's effective uid. Any other peer is logged and dropped.
- **One client:** the companion serves one desktop at a time. A second companion cannot bind the socket while the lock file is held.

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

The client sends `hello` as its **first** frame after connecting. The server replies with its own `hello`, echoing the accepted `protocolVersion`. If `protocolVersion` is unsupported, the **server closes the connection** (the client backs off and retries — typically after one side is upgraded).

```json
{"type":"hello","payload":{
  "protocolVersion": 6,
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
| `protocolVersion` | both | single integer; bumped on any breaking wire change; v4 is deprecated but still accepted |
| `config` | client | desktop's current `GamepadConfig` snapshot (so the companion starts with the right tuning); server omits |
| `mode` | client | desktop's current mode snapshot; server omits |
| `companionSettings` | client | full companion-local settings snapshot, resent on each connection; server omits |
| `gameDetectionRevision` | server | revision of the known-game list the companion has stored (from `game_detection`, kept across restarts); omitted when it has none. The desktop re-sends `game_detection` only when its own revision differs. Additive on v6 |

## Up frames (companion → desktop)

Each maps to an existing webview event; the desktop re-emits the `payload` unchanged.

| `type` | webview event | `payload` shape |
|---|---|---|
| `connection` | `gamepad:connection` | `{ connected: bool, controllerType: string, controllerName: string }` |
| `button` | `gamepad:button` | `{ button: string, pressed: bool, controllerType: string }` |
| `battery` | `gamepad:battery` | `{ percent: i32, charging: bool, wired: bool }` |
| `pads` | `gamepad:pads` | `{ pads: [{ controllerName, controllerType, percent, charging, wired }] }` — every connected controller (multi-controller input), sent when the list or a level changes; additive |
| `cursor_moved` | `gamepad:cursor_moved` | `{ dx: f64, dy: f64 }` |
| `axis` | `gamepad:axis` | `{ leftX: f32, leftY: f32, rightX: f32, rightY: f32 }` |
| `touchpad` | `gamepad:touchpad` | `{ fingers: [{ index: u8, down: bool, x: f32, y: f32, pressure: f32 }] }` |
| `play_sessions` | _(desktop ingest only)_ | `{ sessions: [{ externalId, gameId, startedAt, endedAt, durationMinutes }] }` |
| `settings_changed` | _(desktop ingest only)_ | `{ config: GamepadConfig, companionSettings: CompanionSettings }` |
| `running_games` | _(desktop ingest only)_ | `{ games: [{ gameId: string \| null, pid: u32, exeStem: string, imagePath: string, steamAppId: string \| null, fullscreen: bool }] }` |

Notes:
- `connection.controllerType` / `button.controllerType`: `"xbox" | "playstation" | "switch" | "generic"` (existing desktop vocabulary).
- `battery.percent`: `0–100`, or `-1` when the controller reports no level. `wired` = no internal battery.
- `cursor_moved` / `axis` / `touchpad` are throttled by the companion (≈100 ms) exactly as the desktop poll thread does today.
- `settings_changed` is additive on v6 (do **not** bump `protocolVersion`). The companion emits it when `C:\ProgramData\WarmupVk\settings.ini` (or runtime status) changes on disk — e.g. Control Center or tray edits. A background thread watches the parent directories with `FindFirstChangeNotification` (no mtime polling); the pipe loop only loads an atomic generation counter. External changes are drained before inbound frames each pass. Pipe-originated writes also wake the watcher, but the companion suppresses the echo when the snapshot still matches its SettingsSync baseline. `config` is the companion snapshot (`keyboardTheme` omitted / `null`; `enabled` and `cursorEnabled` both mirror ini `cursor_enabled`); absent optional LED fields mean “not set in ini”.

- `running_games` is additive on v6. The companion sends it while a desktop is connected, at most once a second and only when the list changed; an empty `games` list means no known game runs. It is computed from the connected user's processes:
  - Without a `game_detection` list, an entry is a process owned by a `library_watch` game.
  - With a list, a process is only reported when it is a known game: its normalised image path (trimmed, `\` → `/`, ASCII lowercase) contains one of the `patterns` stored under its file name, or the path contains `/steamapps/common/` and the process's running Steam app id (Windows: `RunningAppID` in the owner's `Software\Valve\Steam` key; Linux: `SteamAppId`, else `SteamGameId`, from the process environment) is in `steamAppIds`, or it runs under one of `localGameDirs`.
  - `gameId` is the `localGameDirs` entry, else the `library_watch` game whose install folder is the longest prefix of the path. Two games tied on the same folder give `null`. A game without folders is matched by exe stem only when that stem belongs to exactly one game and is not generic (`app`, `bootstrap`, `bootstrapper`, `client`, `crashpad_handler`, `game`, `gamelaunchhelper`, `install`, `launch`, `launcher`, `main`, `play`, `run`, `setup`, `start`, `update`, `updater`).
  - Denylisted exes, the background-tool stems and Wallpaper Engine (Steam `431960`) are never reported. Several processes of one game collapse into one entry with the lowest `pid`.
  - `fullscreen` is true when that game owns the foreground window the companion's fullscreen-game heuristic picks. On Linux a Proton game reports its Windows exe as `exeStem` and its `imagePath` resolved through the Wine prefix.

## Down frames (desktop → companion)

| `type` | `payload` shape | Purpose |
|---|---|---|
| `config` | full `GamepadConfig` (below) | push tuning on change (`set_gamepad_config`) |
| `mode` | `{ gameActive: bool, launcherForegroundNav: bool, clicksEnabled: bool, launcherOwnsTextInput: bool, browserActive: bool }` | game-active sleep branch + launcher-foreground nav forwarding (#351), desktop web-VK ownership for native-VK suppression (#353), plus standalone browser ownership for companion-local L3/R3 (native VK / voice) |
| `rumble` | `{ kind: "full", strong: f32, weak: f32, durationMs: u32 }` **or** `{ kind: "triggers", left: f32, right: f32, durationMs: u32 }` | one-shot force feedback (#352) |
| `led` | `{ r: u8, g: u8, b: u8 }` | immediate LED/lightbar write for controller test actions; config still owns persisted/effect state |
| `companion_settings` | `{ sleepOnGame?: bool, autoStopOnGame?: bool, userlandPollPaused?: bool, promptUserlandDebug?: bool }` | companion-local runtime/settings control |
| `library_watch` | `{ enabled: bool, games: [{ gameId, exeStems: string[], installDirPrefixes: string[] }] }` | offline playtime tracking (v6) |
| `play_sessions_ack` | `{ externalIds: string[] }` | offline playtime tracking (v6) |
| `shut_down` | `{}` | ask the companion to stop itself (WAR-42); additive on v6 |
| `game_detection` | `{ revision: u64, chunk: u32, chunks: u32, patterns: string[], steamAppIds: string[], localGameDirs: [{ gameId, prefix }], denylistExes: string[], backgroundToolStems: string[] }` | known-game list for game detection (v6, below) |

### `game_detection` list

warmUP sends its known-game list in one or more chunks so that every NDJSON line stays under 48 KB (the companion drops lines over 64 KB). All chunks of one list carry the same `revision` and `chunks`, and `chunk` counts from `0`. Each chunk carries a slice of the arrays, and the companion appends them in arrival order. It applies the list once every chunk has arrived, then stores it (`game-list.json` beside `library-watch.json`) and replaces the previous list. A chunk for another `revision` restarts collection. A chunk outside `0..chunks`, more than 128 chunks, or a list over the limits (2 MB of text, 20 000 patterns, 30 000 Steam app ids, 4 000 local folders, 4 000 names in each of the two name lists) is rejected and the stored list stays as it was.

| Field | Meaning |
|---|---|
| `patterns` | Discord-style win32 path tails, e.g. `binaries/win64/game.exe`; a leading `>` is ignored, matching is on the normalised path |
| `steamAppIds` | Steam app ids whose games count when they run under `steamapps/common` |
| `localGameDirs` | install folders of manually added games; `prefix` in either slash style, matched case-insensitively |
| `denylistExes` | exe file names that are never games; when non-empty it replaces the companion's built-in list |
| `backgroundToolStems` | exe stems of always-on tools; when non-empty it replaces the built-in `wallpaper32`, `wallpaper64` |

Once a list is stored, the companion's own game signal (sleep on game, standalone game detection) only counts the fullscreen foreground window when its process is a known game by the rules above. Without a list it keeps the plain fullscreen heuristic. The companion also accepts a `game_detection` frame whose fields sit beside `type` instead of inside `payload`.

### `GamepadConfig` payload

camelCase, matching the desktop serde shape:

```json
{
  "deadzone": 0.15,
  "sensitivity": 15.0,
  "accelerationExp": 2.0,
  "scrollSensitivity": 5.0,
  "enabled": true,
  "cursorEnabled": true,
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

`enabled` is the runtime cursor mask on the wire (stick-nav / game leak). It is not the persisted preference when `cursorEnabled` is present.
`cursorEnabled` is the real desktop cursor toggle; the companion persists it to ini `cursor_enabled`. Absent `cursorEnabled` (older desktops) keeps the legacy mapping from wire `enabled` → ini. Companion snapshots set both to the ini value.
`ledEffect` accepts `"solid"`, `"breathing"`, `"rainbow"`, `"gradient"`, or `"off"`.
`gradient` smoothly cycles between `ledColor` and `ledSecondaryColor`.
`ledBrightness` is a `0.0`-`1.0` multiplier applied to all companion-driven LED writes.

Persisted lightbar keys in `C:\ProgramData\WarmupVk\settings.ini` (shared with warmUP):

| ini key | config field | notes |
|---|---|---|
| `led_color` | `ledColor` | `#RRGGBB` |
| `led_secondary_color` | `ledSecondaryColor` | `#RRGGBB` (gradient) |
| `led_effect` | `ledEffect` | `solid` / `breathing` / `rainbow` / `gradient` / `off` |
| `led_brightness` | `ledBrightness` | `0.0`–`1.0` |

Inbound `config` frames persist these when they change (after the first seeding frame). Companion UI / tray writes go through the same keys; startup restores colour, secondary, effect, and brightness from the ini.

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
- `autoStopOnGame`: legacy alias for standalone Guide-only game sleep; the loop stays alive.
- `userlandPollPaused`: runtime pause flag for userland polling; mirrored to runtime status.
- `promptUserlandDebug`: persists the userland prompt debug overlay setting.

When warmUP is connected, warmUP mode frames are authoritative; standalone fullscreen detection
does not override `gameActive` / `launcherForegroundNav`.
warmUP starts with `launcherForegroundNav=true`, so the first connection immediately enters
launcher mode.
Without warmUP, fullscreen detection selects the same sleep mode, so PS/Guide remains
available to wake warmUP.

Sleep mode reads Guide plus warmUP's record chord: Back, RB and Share (SDL `Misc1`, sent as
`QAM`). Those three are only forwarded as `button` frames and never trigger a companion-local
action (no Enter tap, no screenshot, no VK), so a game never sees extra input. warmUP handles them
(tap Share, or hold Back + RB) and does not pass them to the hidden launcher.

Sleep mode also reads LB for one companion-local action: hold Back + LB together for 600 ms to
take one full-screen screenshot (with a rumble). It fires once per hold; release either button to
arm it again. LB is never forwarded while sleeping, and Back is still forwarded as above.

### `shut_down` request (WAR-42)

Additive on protocol v6. Old companions ignore an unknown `type` and keep running. Do **not** bump `protocolVersion` for this frame.

warmUP (desktop, not elevated) sends one NDJSON line after a successful `hello`. Exact wire form, including the empty payload object and the trailing newline:

```
{"type":"shut_down","payload":{}}\n
```

| Field | Required | Notes |
|---|---|---|
| `type` | yes | exactly `shut_down` (snake_case). Not `shutdown`. |
| `payload` | yes | empty object `{}`. Extra camelCase fields are ignored. |

No up-frame ack. Behavior:

1. If Kid Mode blocking is active (`parental_guard.enabled` from the last `parental_guard` / `hello` snapshot), the companion **refuses** the request and stays running. It logs the refusal to the service log. If the parental security log already exists (`C:\ProgramData\WarmupVk\parental-security.log` on Windows, `$XDG_STATE_HOME/warmup-companion/parental-security.log` on Linux), it appends one JSON line: `{"kind":"shut-down-refused","detail":"Kid Mode blocking is active","ts":<unix_secs>}`.
2. Otherwise the companion writes the clean-exit marker `C:\ProgramData\WarmupVk\companion-clean-exit` first (mtime is what warmUP's Kid Mode watchdog treats as a graceful stop; 30s window), then asks SCM to stop `WarmupVkSvc` (`sc stop` from the LocalSystem worker). The service process exits; the client sees the pipe drop.

`Restart App` / updates must not send this frame. Use it only for "Close Companion with warmUP".

The companion maps cursor/scroll tuning fields to its internal names per the golden fixture's `configFieldMapping` (`sensitivity->cursor_speed`, `accelerationExp->cursor_accel`, `deadzone->cursor_deadzone`, `scrollSensitivity->scroll_speed`, plus the optional `scrollAccel->scroll_accel`, `1.0`–`5.0`).

`keyboardTheme` is optional, and each color inside it is optional. Colors are `#RRGGBB`;
the companion persists provided values as `keyboard_bg`, `keyboard_key`,
`keyboard_accent`, `keyboard_text`, and `keyboard_sel_text` in `settings.ini`. Missing
fields keep the native keyboard's current dark/light default for that color slot.

## Versioning policy

- `protocolVersion` is a **single integer**, currently `6`.
- Deprecated protocol versions remain accepted while their frames are wire-compatible; currently accepted deprecated versions: `5`, `4`.
- `browserActive` is an additive v4-compatible mode field; old companions ignore it, new companions use it to keep browser L3/R3 local to native VK/voice.
- While native VK input is unsuppressed (Windows desktop), L3/R3 edges stay local to the companion and are not also emitted as launcher `button` frames. This prevents the same shortcut from opening the keyboard/dictation and the minimized launcher's dock/topbar. Foreground launcher navigation still receives L3/R3; Guide forwarding is unchanged.
- `axis` is an additive v4-compatible up-frame; old desktop clients ignore it as unknown, old companions simply omit it.
- `led` is an additive v4-compatible down-frame for one-shot test writes; old companions ignore it as unknown.
- `shut_down` is an additive v6-compatible down-frame for unelevated companion stop. Old companions ignore it as unknown; Kid Mode blocking refuses it and keeps the service running.
- `game_detection`, `running_games` and `hello.gameDetectionRevision` are additive on v6. Old companions ignore `game_detection` as unknown and never send the revision, so the desktop keeps sending the list on each connection; old desktops ignore `running_games`.
- `settings_changed` is an additive v6-compatible up-frame for companion→desktop settings sync when the shared ini changes locally. Old desktops ignore it as unknown.
- Additive fields may stay on the current protocol when both directions are default/unknown-field tolerant.
- Breaking changes to the pipe name, framing, `hello` shape, or required frame `payload` fields bump it.
- An unsupported version is resolved by the server closing the connection; the client surfaces a "companion update required" state rather than interpreting unknown frames.

## Golden fixtures

Cursor/scroll math parity between the two independent implementations is guarded by a checked-in golden fixture (`(stickX, stickY, dt, config) -> (dx, dy)`). Format + loader are defined in #346 (`src/golden.rs` here, `apps/desktop/src-tauri/src/gamepad/golden.rs` in warmUP); parity assertions land in #349. See `tests/fixtures/cursor-scroll-golden.json`.
