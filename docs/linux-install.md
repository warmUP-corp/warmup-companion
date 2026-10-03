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

If Steam Input grabs the controller, the pad disappears from SDL. The companion logs that and waits for it to come back. It does not try to take the device away from Steam Input.

The virtual pointer is a relative mouse. The compositor's pointer acceleration still applies, so the on-screen distance will not match the raw stick math one-for-one.

There is no on-screen keyboard in this stage. L3 does not open one. Enter is a real key on a second virtual device that only has that key.
