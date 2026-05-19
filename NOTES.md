# Warmup VK Prototype Notes

Question: does UAC/sign-in Xbox VK behavior reduce to desktop eligibility + action-7 gates?

Answer modeled here: yes. Normal `default` instance cannot show Xbox VK on UAC/sign-in; boot/service instance with config `+0xd9` can, if mask `0x200` resolves slot 7 to queued action 7.

## Symbol map (analyzed binary)

| Address | Name | Role |
|---------|------|------|
| `0041eac0` | `warmup_attach_named_desktop` | `OpenDesktopW` + `SetThreadDesktop` (`default` / `winlogon`) |
| `0041ec70` | `warmup_attach_input_desktop` | `OpenInputDesktop` + `SetThreadDesktop` |
| `004199e0` | `warmup_parse_command_line` | CLI; `-boot` → `g_boot_service_mode` |
| `00426080` | `warmup_init_application` | Startup; config `+0xd9` → attach winlogon |
| `00423120` | `warmup_on_controller_press` | Pad down → bindings → `warmup_execute_queued_action` |
| `00423380` | `warmup_on_controller_release` | Pad up → release dispatch |
| `004244f0` | `warmup_process_controller_input` | Resolve physical mask → slot actions |
| `00423510` | `warmup_apply_mask_slot_action` | Mask bit → queue action id (slot 7 = VK) |
| `0044eb60` | `warmup_foreground_timer_proc` | 100 ms timer; foreground / fullscreen profile |
| `00428e00` | `warmup_execute_queued_action` | Run queued action; case 7 = toggle VK |
| `00467190` | `warmup_layout_vk_on_monitor` | Size Xbox VK to monitor |
| `004672b0` | `warmup_create_xbox_vk_window` | Xbox VK window class |
| `00467690` | `warmup_xbox_vk_thread_entry` | Thread wrapper → create Xbox VK |
| `00457560` | `warmup_spiral_vk_thread_entry` | Thread entry → Spiral VK |
| `00456f90` | `warmup_create_spiral_vk_window` | Spiral VK window class |

### Globals

| Address | Name | Role |
|---------|------|------|
| `004a6741` | `g_boot_service_mode` | Set by `-boot` |
| `004a6430` | `g_fullscreen_foreground_flag` | Fullscreen foreground heuristic |
| `004a6684` | `g_app_feature_flags` | Bit 9: Spiral vs Xbox in action 7 |
| `004a74d0` | `g_vk_window_open_latch` | VK open/close toggle |

Rust constants: `src/symbols.rs`.

## SDL3 gamepad (optional)

Uses shared crate **`C:\Users\jonas\warmUp\crates\warmup-gamepad`** (same SDL3 code as warmUP desktop).

```powershell
cargo run --features gamepad -- --gamepad
cargo run --features gamepad   # CLI + `pad` snapshot
```

Maps **Y / Triangle** (SDL north face) → mask `0x200`. Controller DB: `warmUp\apps\desktop\src-tauri\resources\gamecontrollerdb.txt` or env `WARMUP_GAMECONTROLLER_DB`.

## Real on-screen keyboard (`--real`)

**`WarmupXboxVkWindow`** — native UI in `src/win/vk_ui.rs` on a dedicated thread (Joyxoff-style). Keys send `SendInput` to the focused control. No TabTip/osk.

- Desktop attach (`OpenInputDesktop`) runs on the **VK UI thread** before `CreateWindow`, with handles kept open (`desktop.rs`) to avoid `0x800700AA` (resource in use).
- Main/gamepad thread no longer blocks on desktop attach; UAC path uses `VkAttach::Input` on the UI thread.
- If attach fails, the window still opens on the current desktop (logged).

```powershell
cargo run --features gamepad -- --gamepad
```

Gamepad (VK closed): sticks = mouse/scroll, A = click, **tap Y** = open VK.

Gamepad (VK open, borderless overlay, no focus steal):
- **D-pad** — move key focus (orange highlight, hold to repeat)
- **A** — type selected key
- **B** — backspace
- **X** or **tap Y** — close
- **LB** — move caret left in focused field
- **RB** — Enter

**Later:** match Joyxoff layout/theming (`warmup_create_xbox_vk_window`), gamepad navigation inside VK, secure-desktop service (`-boot`).

### UAC / sign-in?

| | Normal app | Boot + winlogon desktop |
|--|------------|-------------------------|
| **Read pad (SDL3)** | Usually yes in same session | Yes if interactive |
| **Show VK** | No (prototype blocks) | Yes |

Pad input ≠ VK window. Original binary uses **XInput**, not SDL.

Run:

```powershell
cargo run --quiet
```

Useful flows:

```text
normal -> fg uac -> press
cfg winlogon on -> boot -> fg logon -> press
cfg winlogon on -> boot -> fg uac -> press -> press
```

## Verifying UAC and sign-in (LogonUI)

This prototype is **not** the real Warmup/Joyxoff Windows service. Secure-desktop tests need **Administrator** (or a future `-boot` service install). SDL gamepad on the logon screen is also unreliable; treat **VK on the secure desktop** as the real test.

### 1. Gate logic only (no secure desktop)

```powershell
cargo run -- --real
```

```text
normal
fg uac
press          # blocked: OpenInputDesktop needs boot
cfg winlogon on
boot
fg uac
press          # simulated path OK; VK uses VkAttach::Input on UI thread
```

### 2. Real VK on your desktop (sanity check)

```powershell
cargo run --features gamepad -- --gamepad
```

Focus Notepad, tap **Y**, D-pad + **A** types. Confirms window + controller path before UAC.

### 3. UAC consent (real secure desktop)

**Why you never see the blue dialog**

1. **Admin shell + `RunAs`** — If you run `Start-Process … -Verb RunAs` from **PowerShell (Admin)**, Windows often elevates **without** showing consent (you already have a full admin token).
2. **Silent UAC for admins** — Registry `ConsentPromptBehaviorAdmin = 0` means *elevate without prompting* for administrators. UAC is on (`EnableLUA = 1`) but **no consent UI**. Check:

```powershell
Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System' |
  Select-Object EnableLUA, ConsentPromptBehaviorAdmin, PromptOnSecureDesktop
```

| `ConsentPromptBehaviorAdmin` | Meaning |
|------------------------------|---------|
| **0** | Admin: elevate silently (no blue screen) |
| **2** | Admin: consent on secure desktop (blue screen) |
| **5** | “Always notify” style for admins |

To get the blue secure-desktop prompt: **Settings → Account → Other security options** (or search “UAC”) → move slider to **Always notify**, sign out/in if asked. Or set `ConsentPromptBehaviorAdmin` to **2** and `PromptOnSecureDesktop` to **1** (requires admin + reboot/logoff per policy).

**Two-window test (when consent prompts are enabled)**

1. **Close** any running `warmup-vk-prototype.exe`.
2. **Window A — Admin:** PowerShell **Run as administrator**, start the prototype:

```powershell
cd C:\Users\jonas\Documents\Codex\2026-05-19\caveman-c-users-jonas-agents-skills
cargo run --features gamepad -- --gamepad --boot --cfg-winlogon
```

3. **Window B — Normal (not Admin):** ordinary PowerShell or **Win+R** → `powershell` (title must **not** say “Administrator”). Then:

```powershell
Start-Process notepad -Verb RunAs
```

Or: Explorer → right-click Notepad → **Run as administrator**.

4. On the **blue UAC dialog**, tap **Y** on the gamepad.

Do **not** run step 3 in the same Admin window as step 2.

**Success looks like:**

- Console: `> vk ui: WarmupXboxVkWindow shown` (no `desktop attach: ... failed` if elevated)
- Borderless keyboard on the **UAC desktop**, topmost, **no focus steal**
- D-pad + **A** types into the UAC field; **RB** = Enter

**Failure looks like:**

- `OpenInputDesktop failed` / `desktop attach: ...` — not elevated, or not `--boot --cfg-winlogon`
- Keyboard on normal desktop behind UAC — attach failed; prototype fell back to `Current` desktop

### 4. Sign-in / LogonUI — Windows service installer

Install once (Admin PowerShell):

```powershell
cd C:\Users\jonas\Documents\Codex\2026-05-19\caveman-c-users-jonas-agents-skills
.\install\Install-WarmupVk.ps1
```

Or manually:

```powershell
cargo build --release --features service
.\target\release\warmup-vk-prototype.exe install
```

This registers **`WarmupVkSvc`** (`LocalSystem`, auto-start) running  
`warmup-vk-prototype.exe --service` → `--boot` + config `+0xd9` (winlogon) + gamepad loop.

| Path | Purpose |
|------|---------|
| `C:\ProgramData\WarmupVk\bin\warmup-vk-prototype.exe` | Service binary |
| `C:\ProgramData\WarmupVk\gamecontrollerdb.txt` | Copied on install (if found) |
| `C:\ProgramData\WarmupVk\service.log` | Service log |

**After install:** reboot (or `Win+L`). At the **password screen**, tap **Y** → VK → type password → **RB** Enter.

**Uninstall (Admin):** `.\target\release\warmup-vk-prototype.exe uninstall`

**Not** “Sign-in options” / Startup apps — those run after login. This is a **boot service** like Joyxoff `-boot`.

**Caveats:** SDL gamepad from a service may fail on some setups (Joyxoff uses XInput). Check `service.log` if Y does nothing at logon.

### 4b. Sign-in simulation (CLI only)

Joyxoff runs as a **boot service** on `winlogon` before LogonUI appears. Without the service, use CLI only:

**CLI gate check only:**

```powershell
cargo run -- --real
cfg winlogon on
boot
fg logon
press
```

**Real logon UI:** requires installing/running as a service at boot (out of scope for this prototype). After that, same as UAC: pad **Y** on the sign-in screen and check `vk ui` logs.

### 5. What to log / check

| Check | Good | Bad |
|-------|------|-----|
| `boot_mode` / `config +0xd9` | `true` | `false` |
| `input desktop` when `fg uac` | `winlogon` | `default` |
| `vk ui: desktop attach` | silent or success | error text |
| `VK window visible` | `true` | `false` |
| Typing into UAC field | characters appear | nothing |

### 6. Limitations (expected)

- Normal non-elevated `cargo run` → VK on **default** desktop only; UAC prompt is another desktop.
- Gamepad on LogonUI without a service → often **no pad** or VK on wrong desktop.
- Full parity = Joyxoff service + XInput + native `JoyXboxVkWindow` from the binary, not this prototype alone.
