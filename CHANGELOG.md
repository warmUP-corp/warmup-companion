# Changelog

## v0.5.0

The keyboard is readable from the couch on a TV. Desk monitors look exactly as before.

- A display that reports a diagonal of 40" or more counts as a TV. On a TV the gamepad hints (L3, R3 voice, Triangle on Space) are 2.25× larger and word suggestions are 5× larger (#43, #44).
- When there is no room above the keyboard, the suggestions stack in a column to the right, next to Enter.
- If the display reports no size, it is treated as a desk monitor. Put `tv` or `desk` in `C:\ProgramData\WarmupVk\display.txt` to override detection, for example behind an AV receiver that reports the wrong size.

## v0.4.1

- Windows sign-in: a Bluetooth PS5 controller's stick click opens the keyboard. The short gamepad report omits that click; the service now reads the fuller Sony report that includes it.

## v0.4.0

The companion follows the monitor you are using, and the voice cloud follows the mic you are using.

- The voice border, prompt, and keyboard sit on the monitor of the focused window. A window that covers every screen uses the monitor under the cursor, then the primary.
- Voice glow scales to this mic's own noise floor. The old pad was sized for a DualSense floor around 0.1 and sat above a normal mic's speech, so the cloud barely moved. A quiet desktop mic and the controller mic both fill the orb. Room tone stays dark. If the first moment of speech is learned as the floor, the next word after a short gap lights it up.
- The cloud gets denser, brighter, and larger with that level. Transcription keeps the glow: the cloud, the screen-edge border, and the mic-key halo all pulse while the engine works.
- The installer skips the voice-engine page and the Parakeet download when the model is already under `C:\ProgramData\WarmupVk\speech\parakeet`.

## v0.3.0

Voice is now the Nimbus cloud (orbkit SHDR-21), drawn on the D3D11 device the
keyboard already uses. Listening and transcription share one frame. The old
ellipse blob and the labelled dictation pill are gone.

- The cloud is a solid disc in the keyboard accent: lit wisps on a darker
  shade of the same colour, so gaps are not transparent and the body is not
  the grey border. Hue does not change between states.
- A clock always steps the noise forward. Talking adds fold inside the shader
  and grows the orb from the quiet size. It does not multiply the running
  clock, which was spinning the field through whole turns. The light only
  drifts.
- Transcription rests at that same quiet size, then swells and settles. The
  shader opens and closes with the swell. Starting sits at the quiet size.
- Keyboard open: the same cloud on the mic key, and only the screen-edge glow
  on the overlay. Keyboard closed: glow plus the cloud.
- The screen edge is an accent glow on the bezel. The outside of the band is
  the screen rectangle, so the corner wedges are filled; only the inner edge
  has a small radius. The frame fades in over 560 ms instead of popping.
- The fullscreen overlay is click-through. Resizing it drops the D2D bitmap
  before `ResizeBuffers`, so a failed resize can no longer leave the last
  frame stuck on screen.
- Cleanup: `main`, the service, the pipe server, parental controls, and
  playtime tracking are split into focused modules, and the XInput backend is
  split into identity, raw HID, secure-desktop polling, and the XInput poll.

## v0.2.19

- Sign-in / lock: keep the companion overlay keyboard. Windows' gamepad PIN
  keyboard (TabTip / CoreInputView Gamepad) is no longer used on builds that
  ship one — it was overlapping the overlay and skipping L3. While LogonUI is
  up, `ControllerToVKMapping` is turned off so focusing the PIN field does not
  summon it. `WARMUP_NATIVE_LOGON_VK=1` still opts back into the OS keyboard.
- Controller-connected card: the pill ⇄ card morph no longer moves or resizes
  the window mid-animation (that was the drift and the missing artwork); both
  states now live on one 720×420 canvas and the card grows upward out of the
  pill. The controller PNG is decoded when the overlay is created, not on the
  first card frame, so the expand no longer stutters. Pill text fades out
  before the title and art fade in. Ready ⇄ "Connect controller" swaps in
  place instead of tearing the window down. `prompt_userland_debug` now loops
  no pad → card → ready every 6.5 s so the morph can be watched on the desktop.
- Controller Center polish: the right-hand tab rail is clickable again
  (SS_NOTIFY labels + icons), colored from the keyboard theme (bg/key/accent/
  text/border, including settings.ini overrides); the selected tab uses the
  accent on a concentric key-colored pill; slider values use tabular
  figure-spaces; checkboxes and footer buttons are 40 px hit targets.
- Install: kill leftover companion workers and retry copying the exe when
  `sc stop` reports STOPPED but `C:\ProgramData\WarmupVk\bin\warmup-companion.exe`
  is still locked.
- Voice orb: peak-normalize mic level against the learned noise floor so
  DualSense / controller speech (quiet, sitting close to a high floor) fills
  the orb instead of barely twitching. Headset mics still saturate at the top.
- Dictation pauses whatever is playing (Spotify, YouTube, and other SMTC
  sessions) for the recording so speaker audio doesn't bleed into the mic, then
  resumes those sessions when transcription finishes.
- Keys now visibly press: the key that fires dips to 96% and settles back over
  150 ms. A/click dips the focused key; the B, Y and Start shortcuts dip the
  Backspace, Space and Enter keys they stand in for, so the badge mapping is
  learned by watching, not reading.
- Suggestion strip polish: a soft layered shadow replaces the hard offset
  copy, the highlighted word sits concentrically inside the pill, and the
  strip fades up into place (120 ms) when a word starts offering candidates.
  Dismissal stays instant.
- Floating keyboard card: corner radius is now derived from the key radius plus
  the card padding (concentric corners), and the key block gets the same 18 px
  inset at the bottom as at the sides.
- Dictation UI: the keyboard-closed voice indicator is now a labelled pill
  (orb, phase title, and the controller's R3 glyph with "Stop" while
  listening) instead of a bare orb, so the phase is readable without watching
  the motion. "Starting…" is shown distinctly while the helper spins up, since
  speech in that gap is not captured. The pill fades and scales in (160 ms),
  fades out (120 ms) instead of blinking away, and phase changes swap the title
  in place instead of recreating the window. Mic level uses one fast-attack /
  slow-release envelope on both the pill and the keyboard's mic key, and the
  transcribing pulse is brisker.
- Motion values for the keyboard live in one place (`src/vk_motion.rs`) with
  unit tests, instead of scattered constants.

## v0.2.18

- Opening the native keyboard with L3 or starting dictation with R3 on the Windows
  desktop no longer also sends that shortcut to warmUP's dock/topbar, which could
  bring the minimized launcher over the app being typed into.

## v0.2.17

- Yield the sign-in screen to Windows' native controller keyboard on Windows 11
  builds 26100.4762 and newer: no prompt card, no companion keyboard, and no
  native-panel suppression while LogonUI owns the secure desktop. Xbox pads are
  handled by Windows; PlayStation pads get their buttons translated into the
  PIN legend keystrokes, with A/Start as Enter and B as Escape on dialogs
  without a PIN field. UAC and other secure prompts keep the previous behavior.
  Override with `WARMUP_NATIVE_LOGON_VK=1|0`.

## v0.2.16

- Do not resize the warmUP Game Launcher when the docked keyboard opens after
  Guide-close; keep the last eligible app window instead.

## v0.2.15

- Show the Companion version and build checksum in its taskbar tray UI.

## v0.2.14

- Honor warmUP's gamepad-cursor master switch for stick, touchpad, scrolling,
  and A/B mouse clicks while retaining launcher navigation.
- Give the canonical cursor setting precedence over its legacy alias.

## v0.2.13

- Keep cursor movement and clicks in the same mode so the desktop never gets
  stuck with movement-only controls.
- Let warmUP own game/launcher/desktop input state while connected, while
  retaining standalone game detection.
- Drain large desktop configuration frames without blocking later mode updates.
- Respect warmUP's native-keyboard suppression for the R3 voice shortcut.

## v0.2.12

- Restore full controller input in warmUP and on the Windows desktop while
  keeping active games Guide-only.

## v0.2.11

- Restore warmUP release IPC access while retaining per-user and per-session
  ownership for offline tracking.
- Launch warmUP only with a complete active-user environment, preventing broken
  WebView startup under service/SYSTEM paths.
- Keep controller input asleep while warmUP or a game owns the foreground.
- Add an explicit development install mode for trusted local warmUP builds.

## v0.2.10

- Keep controller input Guide-only while a game owns the foreground.

## v0.2.9

- Restore the desktop-only warmUP launch chord: it is blocked while warmUP is
  foreground or a game is active.

## v0.2.8

- Harden offline tracking IPC with canonical executable authentication,
  SID/session ownership, connection-scoped acknowledgements, and bounded watch
  payloads.
- Filter playtime process discovery to the authenticated Windows user and
  session.
- Maximized foreground windows now reflow above the keyboard and return to
  maximized state when it closes.

## v0.2.7

- IPC protocol v6 adds offline playtime tracking while the warmUP desktop is
  disconnected.
- The desktop can push a library watchlist, receive completed offline sessions,
  and acknowledge persisted sessions after reconnecting.
- Pending playtime and library-watch state survive companion restarts.

## v0.2.6

- Bigger native prompts: the "Connect controller" / "Press [L3] for keyboard"
  pills and the controller-connected card are ~2x larger for 10-foot (TV) use.
- Fix: warmUP theme colors now reach the native prompts. Settings moved to the
  fixed `C:\ProgramData\WarmupVk\settings.ini` so the SYSTEM service that handles
  the IPC config push and the one that renders the prompts share the same file
  (previously split by an unreliable `%LOCALAPPDATA%` under LocalSystem).
- Fix: the launch hotkey no longer buzzes a fake "launched" confirmation when
  warmUP isn't installed — it gives a softer tick and logs the skipped launch.

## v0.2.5

- Polished the controller keyboard, tray menu, voice input, game detection, and branded installer experience.
- Silent installs now include offline Parakeet voice typing by default and run helper processes without terminal windows.
- Uninstall now fails safely when the service cannot stop, removes stale installed assets, and stays silent.
- Tray **Exit** now stops `WarmupVkSvc` instead of allowing the service worker to relaunch.
- The installer finish page now prominently links to the warmUP Game Launcher.

## v0.2.4

- Kid Mode companion IPC v5 support with system-wide game blocking notifications.
- Installer metadata now reports the release version.

## v0.2.3

- Browser mode: accept warmUP desktop's `browserActive` mode bit so L3/R3 stay
  companion-local while the standalone browser is foreground.
- Fix: R3 voice dictation is allowed for the warmUP browser/overlay but remains
  blocked for the main launcher to prevent accidental transcript injection.

## v0.2.2

- Fix: the installer's service-install-failure dialog had no `/SD` flag, so it
  could still pop up and block forever during a silent (`/S`) install with no
  one to click it. Defaults to `IDOK` under `/S` instead.

## v0.2.1

- Release checksum sidecars: both `warmup-companion.exe` and
  `warmup-companion-setup.exe` now ship with a matching `.sha256` file for
  desktop in-app install verification.

## v0.2.0

- Controller right-click + Share→Enter on the secure-desktop on-screen
  keyboard path.
- Fix "Missing" companion status in the warmUP app by granting `Users`
  read+execute on the install `bin` dir and writing a version marker.
- Speech/Parakeet, VK, and tray refinements.

## v0.0.1

- First tagged release. Controller-driven on-screen keyboard via `SendInput`,
  working on UAC, lock, and sign-in surfaces. Local English-only prefix
  prediction (disabled on secure surfaces). Sleeps the controller loop while
  a game owns the pad.
