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

## Update

```sh
warmup-companion update --check
warmup-companion update
```

`--check` shows whether a newer release is out and whether it has a Linux build. `update` downloads the Linux build, checks it against the `.sha256` file published next to it, replaces the installed binary, and restarts `warmup-companion.service` if it is running. A service you stopped stays stopped and runs the new version the next time it starts. Settings, the log, and the unit file are not touched. If the download or the checksum fails, the installed binary stays as it was. The previous binary is kept next to it as `warmup-companion.old`, and `update` prints the command that puts it back.

`warmup-companion update --dry-run` downloads and checks the new build, then deletes it without replacing anything.

Releases do not carry a Linux build yet. Until they do, `update` says the release has no Linux build and changes nothing. To update before then, pull the source, build it again, and run `./target/release/warmup-companion install`. Installing over an existing install replaces the binary; run `systemctl --user restart warmup-companion` afterwards.

While `update_check` is on in `settings.ini` (the default), the service checks shortly after it starts and once a day after that. When a newer release has a Linux build, it shows a notification with Install and Later buttons, like on Windows. Install, or a click on the notification, opens the update window. The same window opens from the tray under Updates: it shows what is new, the download and checksum progress, and the Check automatically switch, and the controller drives it with the D-pad, A and B. Turn the check off with `warmup-companion settings set update_check false`.

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
| Installed binary | `$XDG_DATA_HOME/warmup-companion/bin/warmup-companion` |
| Input socket | `$XDG_RUNTIME_DIR/warmup/input.sock` |

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

L3 opens the on-screen keyboard on a layer above other windows. It does not take keyboard focus, so typing still goes to the window you were using. Docked mode sits on the bottom edge and reserves that strip; floating mode stays centred. `warmup-companion type <text>` types into the focused window without opening the keyboard, including characters outside the current layout. Enter on the stick is still its own key, separate from that typing device. The lock screen cannot show the keyboard.

## Tray and settings

The running service adds a StatusNotifierItem. Left-click it for the companion menu, the same one Windows shows: pause input, Control Center, keyboard settings, game detection, voice typing, updates, the log, diagnostics, uninstall and exit. The D-pad, A and B work in it, and a click outside or Esc closes it. Right-click shows the same settings as a plain menu. Select + Start on the controller also opens and closes Control Center, and the controller drives it while it is open.

Ctrl+Alt+C opens and closes Control Center and Ctrl+Alt+V starts or stops voice typing, like on Windows. The companion registers both through the desktop's global shortcuts portal (`org.freedesktop.portal.GlobalShortcuts`), so KDE Plasma lists them in its shortcut settings, where they can be changed. xdg-desktop-portal-hyprland registers them without a key, so on Hyprland the companion also binds Ctrl+Alt+C and Ctrl+Alt+V to them with `hyprctl` while it runs. A combo you already bound yourself is left alone. Ctrl+Alt+V is bound only while voice typing is on. For the portal the companion writes a hidden `warmup-companion.desktop` entry to `~/.local/share/applications`. Without the portal, the Hyprland binds signal the systemd service directly. The tray shows Ctrl+Alt+C next to Control Center when the shortcut is active.

`warmup-companion settings` reads and writes `$XDG_CONFIG_HOME/warmup-companion/settings.ini`. `warmup-companion --controller-center` opens Control Center on its own, without the controller. Like on Windows, it adds its own tray icon and keeps running after the window closes, so the tray can open it again; stop it with Ctrl+C. `warmup-companion diagnostics` writes a tarball under `$XDG_STATE_HOME/warmup-companion/diagnostics/`.

## Hyprland-only features

These v0.10 features read Hyprland's IPC socket, so they work only on Hyprland:

- **Per-app button shortcuts.** The focused app comes from Hyprland's active window. On KDE Plasma and other desktops no app is ever focused for the companion, so buttons keep their default actions.
- **Workspace swipes.** A sideways touchpad swipe moves to the previous or next workspace on Hyprland. Swipe up and down do nothing there. On KDE Plasma, swipes send Plasma's shortcuts instead (Ctrl+Meta+Left/Right, Meta+W, Meta+D). Other desktops ignore swipes.
- **Pause input while a game is running.** See below.

On Hyprland, "Pause input while a game is running" sleeps the pad while the focused window is fullscreen, or a floating window covers the monitor, unless that program is a browser, launcher, or shell. KDE Plasma's KWin does not advertise `zwlr_foreign_toplevel_management_v1`, so the same switch does not see games there.
