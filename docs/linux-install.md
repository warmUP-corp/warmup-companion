# Linux install

The companion runs as a systemd user service. It reads the controller with SDL3 and moves the pointer, scrolls, clicks, and presses Enter through a virtual input device. warmUP does not have to be running.

This build targets KDE Plasma on gaming distributions (SteamOS desktop mode, Bazzite, CachyOS, ChimeraOS, Nobara) and Hyprland. GNOME is not supported.

Install stays inside your home folder. The binary is not copied to `/usr`, which is read-only on SteamOS and image-based on Bazzite.

## Install

Build the release binary, then install it for the current user:

```sh
cargo build --release
./target/release/warmup-companion install
```

That copies the binary to `$XDG_DATA_HOME/warmup-companion/bin/warmup-companion` (usually `~/.local/share/warmup-companion/bin/warmup-companion`), writes `~/.config/systemd/user/warmup-companion.service`, and runs `systemctl --user daemon-reload` and `systemctl --user enable --now warmup-companion.service`.

The unit is `Type=simple`, `Restart=on-failure`, `RestartSec=2`, `PartOf=graphical-session.target`, and `WantedBy=graphical-session.target`. It starts with `warmup-companion --service`. Service mode is that flag. Closing the terminal does not start the service.

Check it:

```sh
warmup-companion verify
systemctl --user status warmup-companion
```

Stop it with `warmup-companion stop`. Remove the unit and the installed binary with `warmup-companion uninstall`. Settings, the log, and the socket directory are left in place.

## Finding warmUP

The Guide (PS) button opens warmUP when it is closed, unless *Guide / PS opens warmUP when closed* is off in Control Center. The View + LB + X shortcut opens it too. From the service, warmUP starts through `systemd-run --user` in its own unit, so restarting the companion does not close it. The companion looks for the launcher in this order and uses the first executable file it finds:

1. `WARMUP_EXE`, if set.
2. The path written in `$XDG_DATA_HOME/warmup-companion/warmup-exe.path` by `install-dev`.
3. `warmup` next to the installed companion binary.
4. `$XDG_DATA_HOME/warmup/warmup`.
5. `~/.local/bin/warmup`, then `~/.local/bin/warmup.AppImage` (where the warmUP Linux guide puts the AppImage).
6. `/usr/bin/warmup` (the `.deb` and `.rpm` packages), then `/usr/local/bin/warmup`.
7. A `warmup` on `PATH`.

To point the companion at a warmUP you built from source, install it in DEV mode:

```sh
./target/release/warmup-companion install-dev ~/Projects/warmUP/apps/desktop/src-tauri/target/debug/warmup
```

That installs like `install` and also writes the path to `warmup-exe.path`. The target must be a file named `warmup` or a warmUP AppImage. A plain `install` or `uninstall` removes the DEV path again.

## Update

```sh
warmup-companion update --check
warmup-companion update
```

`--check` shows whether a newer release is out and whether it has a Linux build. `update` downloads the Linux build, checks it against the `.sha256` file published next to it, replaces the installed binary, and restarts `warmup-companion.service` if it is running. A service you stopped stays stopped and runs the new version the next time it starts. Settings, the log, and the unit file are not touched. If the download or the checksum fails, the installed binary stays as it was. The previous binary is kept next to it as `warmup-companion.old`, and `update` prints the command that puts it back.

`warmup-companion update --dry-run` downloads and checks the new build, then deletes it without replacing anything.

Releases do not carry a Linux build yet. Until they do, `update` says the release has no Linux build and changes nothing. To update before then, pull the source, build it again, and run `./target/release/warmup-companion install`. Installing over an existing install replaces the binary; run `systemctl --user restart warmup-companion` afterwards.

While `update_check` is on in `settings.ini` (the default), the service checks shortly after it starts and once a day after that. When a newer release has a Linux build, it shows a notification with the full path of the installed program, since `~/.local/share/warmup-companion/bin` is usually not on `PATH`. Turn the check off with `warmup-companion settings set update_check false`.

## Session environment

The user service needs `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` in the systemd user manager. A session started by `uwsm` already exports them. On a plain Hyprland session, import them before expecting the service to stay up:

```sh
dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_RUNTIME_DIR
systemctl --user restart warmup-companion
```

If either variable is missing, the service logs the reason and exits. `verify` also reports a missing `WAYLAND_DISPLAY`.

## Paths

| What | Path |
| --- | --- |
| Settings | `$XDG_CONFIG_HOME/warmup-companion/settings.ini` |
| Log and runtime status | `$XDG_STATE_HOME/warmup-companion/` |
| Library watchlist and unsent play sessions | `$XDG_STATE_HOME/warmup-companion/library-watch.json`, `pending-play-sessions.json` |
| Kid Mode block list | `$XDG_STATE_HOME/warmup-companion/parental-guard.json` |
| Crash report | `$XDG_STATE_HOME/warmup-companion/crash-report.txt`, `crash-report.last.txt` once read |
| Installed binary | `$XDG_DATA_HOME/warmup-companion/bin/warmup-companion` |
| Controller database | `$XDG_DATA_HOME/warmup-companion/gamecontrollerdb.txt` |
| Input socket | `$XDG_RUNTIME_DIR/warmup/input.sock` |

`install` copies the SDL controller database from `WARMUP_GAMECONTROLLER_DB`, or else from the warmUP launcher it finds (its `resources/gamecontrollerdb.txt`, or `/usr/lib/warmUP/resources/` for the `.deb` and `.rpm`). Without it, the companion uses the controller mappings built into SDL.

The socket directory is mode `0700` and the socket is mode `0600`. The companion accepts a client only when that client's user id matches its own. The desktop connects to this socket with the same protocol version 6 frames the Windows build uses on its named pipe.

## `/dev/uinput`

SteamOS, Bazzite, CachyOS, ChimeraOS, and Nobara already let your user open `/dev/uinput` through Steam's udev rule. No extra rule is required there.

On any other distribution, install the bundled rule and reload udev. This step needs root and writes under `/usr`, so the user-local installer does not do it:

```sh
sudo cp install/70-warmup-companion.rules /etc/udev/rules.d/70-warmup-companion.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Log out and back in so the new device permission applies. `verify` checks that `/dev/uinput` is writable.

## Controller battery and lightbar (`/dev/hidraw*`)

The battery level and the lightbar colour of a DualSense or DualShock 4 go through SDL's HIDAPI driver, which talks to the pad's `/dev/hidraw*` node. Without access to that node the sticks and buttons still work, but the battery shows as unknown and the lightbar does not change.

Steam's udev rules already give your user that access. They ship with Steam on SteamOS, Bazzite, CachyOS, ChimeraOS, and Nobara, and as `steam-devices` (or `steam-udev`) on most other distributions. The bundled `install/70-warmup-companion.rules` grants the same access for the DualSense, DualSense Edge, and DualShock 4, over USB and Bluetooth, so installing it as shown above covers both `/dev/uinput` and the controller. Unplug and reconnect the controller after installing a rule.

Check it with the controller connected:

```sh
ls -l /dev/hidraw*
getfacl /dev/hidrawN
```

The node for the controller should list your user with `rw`.

If Steam Input grabs the controller, the pad disappears from SDL. The companion logs that and waits for it to come back. It does not try to take the device away from Steam Input.

The virtual pointer is a relative mouse. The compositor's pointer acceleration still applies, so the on-screen distance will not match the raw stick math one-for-one.

L3 opens the on-screen keyboard on a layer above other windows. It does not take keyboard focus, so typing still goes to the window you were using. A mouse click or a tap on a key or a suggestion works too. Docked mode sits on the bottom edge and reserves that strip; floating mode stays centred on the bottom edge. `warmup-companion type <text>` types into the focused window without opening the keyboard, including characters outside the current layout. Enter on the stick is still its own key, separate from that typing device. The lock screen cannot show the keyboard. The first time a controller connects after an update, the controller tips pill from Windows walks through the main shortcuts at the bottom of the screen.

## Tray and settings

The running service adds a StatusNotifierItem. Left-click it for the companion menu, the same one Windows shows: pause input, Control Center, keyboard settings, game detection, voice typing, updates, the log, diagnostics, uninstall and exit. The D-pad, A and B work in it, and a click outside or Esc closes it. Right-click shows the same settings as a plain menu. Select + Start on the controller also opens and closes Control Center, and the controller drives it while it is open.

While warmUP is closed, the companion counts play time for the games in the watchlist warmUP last sent. It reads `/proc` every two seconds and only looks at your own processes. A game matches by its program name or by its install folder; for a Wine or Proton game the program name is the Windows `.exe` name. A session ends ten seconds after the game is gone and stays in `pending-play-sessions.json` until warmUP connects and confirms it.

While Kid Mode blocking is on, the companion checks running programs every two seconds, matching them the same way, and stops any blocked game with SIGTERM, then SIGKILL 0.8 seconds later. The service runs as your user, so it can only stop games running under your own account. This is intended: on Windows the service runs as SYSTEM and can stop a blocked game started by any account, but on Linux a game started by another user is not stopped. warmUP is still told that a blocked game is running. Most games end at once on SIGTERM, without a chance to save.

When warmUP asks the companion to shut down while Kid Mode is blocking, the companion refuses. If `$XDG_STATE_HOME/warmup-companion/parental-security.log` already exists, it also appends one JSON line for the refused request.

`warmup-companion settings` reads and writes `$XDG_CONFIG_HOME/warmup-companion/settings.ini`. `warmup-companion --controller-center` opens Control Center on its own, without the controller. `warmup-companion diagnostics` writes a tarball under `$XDG_STATE_HOME/warmup-companion/diagnostics/`. It holds the unit status, the journal, the service log, device and Hyprland details, the watchlist, the unsent play sessions, the Kid Mode block list and both crash reports.

## Screenshots

LB + RB + RT takes a screenshot of every screen, and LB + RB + LT takes one of the focused window. While a game has the controller, holding View + LB takes a screenshot of every screen. Each one is copied to the clipboard, saved as `Screenshot <date> <time>.png` (`... window.png` for a window) in `Screenshots` under your Pictures folder (`XDG_PICTURES_DIR`, usually `~/Pictures`), and announced with a notification that opens the file.

The companion captures with `grim` and copies with `wl-copy`:

```sh
sudo pacman -S grim wl-clipboard        # Arch / Omarchy
sudo apt install grim wl-clipboard      # Debian / Ubuntu
sudo dnf install grim wl-clipboard      # Fedora
```

The focused window is found through Hyprland. On other desktops, or when `grim` cannot capture (KDE Plasma), the companion asks the desktop's screenshot portal instead. The portal always captures the whole screen and may ask for permission the first time.

## Paste into a terminal

View + Y and the keyboard's paste key send Ctrl+Shift+V when the focused window is a terminal (Alacritty, kitty, foot, WezTerm, Ghostty, Konsole, GNOME Terminal), and Ctrl+V everywhere else. When the clipboard holds a picture or copied files instead of text, the terminal gets their quoted paths typed in; a picture is saved to Pictures/Screenshots first. This reads the clipboard with `wl-paste` from `wl-clipboard`. The focused window is known on Hyprland and on compositors with `zwlr_foreign_toplevel_management_v1`.

## Hyprland-only features

These v0.10 features read Hyprland's IPC socket, so they work only on Hyprland:

- **Per-app button shortcuts.** The focused app comes from Hyprland's active window. The built-in presets match the Linux apps too: VLC, Spotify, Dolphin, Nautilus, Thunar and Nemo as File Explorer, and Discord, Vesktop and Legcord as Discord. A setting saved for the Windows name (for example `profile.explorer.exe.B`) applies to them as well. On KDE Plasma and other desktops no app is ever focused for the companion, so buttons keep their default actions.
- **Workspace swipes.** A sideways touchpad swipe moves to the previous or next workspace on Hyprland. Hyprland has no task view or show-desktop of its own, so swipe down moves to an empty workspace on the current screen, which shows the desktop, and swipe up goes back to the workspace you were on before on that screen. Swipe down does nothing when the current workspace is already empty. On KDE Plasma, swipes send Plasma's shortcuts instead (Ctrl+Meta+Left/Right, Meta+W, Meta+D). Other desktops ignore swipes.
- **Pause input while a game is running.** See below.

On Hyprland, "Pause input while a game is running" sleeps the pad while the focused window is fullscreen, or a floating window covers the monitor, unless that program is a browser, launcher, or shell. KDE Plasma's KWin does not advertise `zwlr_foreign_toplevel_management_v1`, so the same switch does not see games there.
