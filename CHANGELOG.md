# Changelog

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
