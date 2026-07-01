# Changelog

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
