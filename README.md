<p align="center">
  <img src="assets/promo/warmup-logo-wordmark.png" alt="warmUP" width="360">
</p>

# warmUP Companion

Type anywhere in Windows with the controller already in your hands. warmUP Companion is an open-source controller keyboard for Xbox and PlayStation controllers, including desktop apps, UAC prompts, the lock screen, and sign-in screen.

[Download for Windows](https://github.com/warmUP-corp/warmup-companion/releases/latest)
· [Join the Discord](https://discord.gg/z2zmB4v2Bn)
· [Report a problem or idea](https://github.com/warmUP-corp/warmup-companion/issues/new/choose)
· [warmUP Game Launcher](https://www.warmup-gamelauncher.com/)

## Features

- Works on Windows without Steam or the warmUP Game Launcher.
- Opens a native on-screen keyboard and shows Xbox or PlayStation button hints.
- Types into desktop apps, UAC prompts, lock screens, and sign-in screens.
- Offers local English word suggestions without reading text from the focused app.
- Supports optional offline voice typing with Whisper.cpp or NVIDIA Parakeet.
- Sleeps controller input while a game is active, then wakes for desktop use.

![Controller-driven virtual keyboard in warmUP on Windows](assets/promo/warmup-keyboard-library.png)

<sub>Press L3 (left-stick click) to open the keyboard.</sub>

---

## Installation

1. Download `warmup-companion-setup.exe` from the [latest release](https://github.com/warmUP-corp/warmup-companion/releases/latest).
2. Run it and approve the UAC prompt. The installer adds the Windows service required for secure Windows surfaces.
3. Connect a controller and press **L3** to open the keyboard.

No Steam setup is required. To try the sign-in path after confirming normal desktop typing, press `Win+L` and use the controller from the lock screen.

### Supported controllers

- Xbox 360 and Xbox One-family controllers
- PlayStation DualShock 4
- PlayStation DualSense and DualSense Edge
- Other compatible HID gamepads recognized through the controller mapping database

## Controller controls

### Keyboard closed

| Xbox | PlayStation | Action |
| --- | --- | --- |
| Left stick | Left stick | Move the mouse pointer |
| Right stick | Right stick | Scroll |
| <img src="controller-icons/x_face_a_colored.svg" alt="A" width="28" align="middle"> | <img src="controller-icons/p5_face_cross_colored.svg" alt="Cross" width="28" align="middle"> | Click |
| <img src="controller-icons/x_l3_click.svg" alt="L3" width="28" align="middle"> | <img src="controller-icons/p5_l3_click.svg" alt="L3" width="28" align="middle"> | Open the keyboard |

### Keyboard open

| Xbox | PlayStation | Action |
| --- | --- | --- |
| D-pad or left stick | D-pad or left stick | Move keyboard focus |
| <img src="controller-icons/x_face_a_colored.svg" alt="A" width="28" align="middle"> | <img src="controller-icons/p5_face_cross_colored.svg" alt="Cross" width="28" align="middle"> or touchpad | Type the selected key |
| <img src="controller-icons/x_face_b_colored.svg" alt="B" width="28" align="middle"> | <img src="controller-icons/p5_face_circle_colored.svg" alt="Circle" width="28" align="middle"> | Backspace |
| <img src="controller-icons/x_face_x_colored.svg" alt="X" width="28" align="middle"> | <img src="controller-icons/p5_face_square_colored.svg" alt="Square" width="28" align="middle"> | Switch keyboard language/layout |
| <img src="controller-icons/x_face_y_colored.svg" alt="Y" width="28" align="middle"> | <img src="controller-icons/p5_face_triangle_colored.svg" alt="Triangle" width="28" align="middle"> | Space |
| <img src="controller-icons/x_menu_menu.svg" alt="Menu" width="28" align="middle"> | <img src="controller-icons/p5_options.svg" alt="Options" width="28" align="middle"> | Enter |
| <img src="controller-icons/x_menu_view.svg" alt="View" width="28" align="middle"> | <img src="controller-icons/p5_share.svg" alt="Create or Share" width="28" align="middle"> | Focus and advance word suggestions |
| <img src="controller-icons/x_shoulder_lb.svg" alt="LB" width="36" align="middle"> / <img src="controller-icons/x_shoulder_rb.svg" alt="RB" width="36" align="middle"> | <img src="controller-icons/p5_shoulder_l1.svg" alt="L1" width="36" align="middle"> / <img src="controller-icons/p5_shoulder_r1.svg" alt="R1" width="36" align="middle"> | Cycle word suggestions |
| <img src="controller-icons/x_menu_view.svg" alt="View" width="28" align="middle"> + <img src="controller-icons/x_face_x_colored.svg" alt="X" width="28" align="middle"> / <img src="controller-icons/x_face_y_colored.svg" alt="Y" width="28" align="middle"> / <img src="controller-icons/x_face_b_colored.svg" alt="B" width="28" align="middle"> | <img src="controller-icons/p5_share.svg" alt="Create or Share" width="28" align="middle"> + <img src="controller-icons/p5_face_square_colored.svg" alt="Square" width="28" align="middle"> / <img src="controller-icons/p5_face_triangle_colored.svg" alt="Triangle" width="28" align="middle"> / <img src="controller-icons/p5_face_circle_colored.svg" alt="Circle" width="28" align="middle"> | Copy / paste / clear the focused input |
| <img src="controller-icons/x_trigger_lt.svg" alt="LT" width="36" align="middle"> | <img src="controller-icons/p5_trigger_l2.svg" alt="L2" width="36" align="middle"> | Symbols |
| <img src="controller-icons/x_trigger_rt.svg" alt="RT" width="36" align="middle"> | <img src="controller-icons/p5_trigger_r2.svg" alt="R2" width="36" align="middle"> | Shift |
| <img src="controller-icons/x_l3_click.svg" alt="L3" width="28" align="middle"> | <img src="controller-icons/p5_l3_click.svg" alt="L3" width="28" align="middle"> | Close the keyboard |
| <img src="controller-icons/x_r3_click.svg" alt="R3" width="28" align="middle"> | <img src="controller-icons/p5_r3_click.svg" alt="R3" width="28" align="middle"> | Start or stop optional voice typing |

## Settings

| Setting | Default | What it does |
| --- | --- | --- |
| Game sleep | On | Stops companion mouse and keyboard input while a fullscreen game is active. Press Guide or PS to wake it. |
| Voice typing | Off until installed | Uses an optional local speech engine on the normal desktop. It is unavailable on lock and sign-in screens. |
| Suggestions | On | Keeps a local, English-only prefix buffer from characters typed by the companion. |

Use the tray icon to check status and privacy settings. To manage game sleep from a terminal:

```powershell
warmup-companion.exe settings sleep-on-game get
warmup-companion.exe settings sleep-on-game off
warmup-companion.exe settings sleep-on-game on
```

## Privacy and security

- Key input is injected through Win32 `SendInput`.
- Suggestions do not read the text in the focused application. They only use characters typed by this keyboard.
- Suggestions and learning are disabled on UAC, lock, and sign-in surfaces.
- Personal-dictionary writes are skipped for password fields. If password detection fails, learning is skipped.
- Crash telemetry is off unless `WARMUP_SENTRY_DSN` is explicitly set. Local crash dumps are never uploaded.

The service runs as `WarmupVkSvc` under LocalSystem because secure-desktop input is not available to a normal desktop application. Read the [privacy policy](PRIVACY.md) and [security policy](SECURITY.md) before installing.

## Advanced installation and diagnostics

### From source

From an Administrator PowerShell:

```powershell
.\install\Install-WarmupVk.ps1
```

This builds the release binary, installs `WarmupVkSvc`, and writes logs to `C:\ProgramData\WarmupVk\service.log`.

### Uninstall

If you used the installer, remove **warmUP Companion** in **Settings > Apps**, or run:

```powershell
"C:\Program Files\WarmupCompanion\uninstall.exe" /S
```

For a source install, run this from an Administrator PowerShell:

```powershell
.\target\release\warmup-companion.exe uninstall
```

### Diagnostics

```powershell
.\install\Collect-WarmupVkDiagnostics.ps1
```

The report includes service status, installed binary metadata, recent service logs, and HID/Winlogon signal lines. Review local crash dumps before sharing them.

## Build

```powershell
cargo build --release
cargo check
```

- [Contributing](CONTRIBUTING.md)
- [Domain glossary](CONTEXT.md)
- [IPC protocol](docs/companion-ipc-protocol.md)
- [Release process](docs/RELEASES.md)
