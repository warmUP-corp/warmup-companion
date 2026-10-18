# Linux self-update

This is a plan only. The Windows updater in `src/updater.rs` stays Windows-only. Do not port that module, and do not download or replace the binary from this document.

## What Windows does today

`src/updater.rs` asks GitHub for the latest release:

`https://api.github.com/repos/warmUP-corp/warmup-companion/releases/latest`

It compares `tag_name` (without a leading `v`) to the crate version as `major.minor.patch`. A newer release is one whose triple is greater. The check runs only while `update_check` is on in `settings.ini`. The first check waits 15 seconds after start. A failed check tries again after 10 minutes. A successful check waits a day.

The Windows asset is `warmup-companion-setup.exe`, plus a sibling `warmup-companion-setup.exe.sha256`. Both URLs must start with:

`https://github.com/warmUP-corp/warmup-companion/releases/download/`

The download is checked with SHA-256 before a silent NSIS install. Release notes keep up to three `- ` lines from the GitHub body. The tray and the update window talk to the service through named Win32 events. None of that UI exists on Linux, and it should not be copied.

## What a Linux update has to install

The home-folder install in `docs/linux-install.md` and `src/install_linux.rs` is the target. There is no root, and nothing is written under `/usr`. That matters on SteamOS and Bazzite, where `/usr` is read-only or image-based.

| What | Path |
| --- | --- |
| Installed binary | `$XDG_DATA_HOME/warmup-companion/bin/warmup-companion` (usually `~/.local/share/warmup-companion/bin/warmup-companion`) |
| User unit | `~/.config/systemd/user/warmup-companion.service` |
| Settings | `$XDG_CONFIG_HOME/warmup-companion/settings.ini` |
| Log and runtime status | `$XDG_STATE_HOME/warmup-companion/` |
| Input socket | `$XDG_RUNTIME_DIR/warmup/input.sock` |

The unit is `Type=simple`, `Restart=on-failure`, `RestartSec=2`, `PartOf=graphical-session.target`, and `WantedBy=graphical-session.target`. `ExecStart` is that binary with `--service`.

Settings, the log, the runtime status, and the socket stay in place. Uninstall already leaves them. An update must leave them too.

`update_check` is already a shared setting. The Linux check honours the same key the Windows updater reads.

## The release asset does not exist yet

GitHub releases do not publish a Linux binary today. The Windows updater requires `warmup-companion-setup.exe`. A Linux updater must not download that installer.

A release would need a Linux asset under the same download prefix, plus a sibling `.sha256` file. The asset name is not chosen yet. Until that file is on the release, the check should say there is no Linux build. It should not fall back to the Windows installer.

Both URLs stay restricted to `https://github.com/warmUP-corp/warmup-companion/releases/download/`. Verify SHA-256 before the new bytes replace anything. A mismatch deletes the download and leaves the installed binary as it is.

## Replacing a running binary

The service is the process executing the installed binary. Opening that file for writing fails with `ETXTBSY`, but `rename()` over it succeeds: the running process keeps the old inode until it exits.

Download next to it, for example `warmup-companion.new`, in the same directory. Verify the checksum there. Then a short helper that is not the running binary should:

1. `systemctl --user stop warmup-companion.service`
2. Rename `warmup-companion.new` onto `warmup-companion` (keep the previous file only until the rename succeeds)
3. `systemctl --user start warmup-companion.service`

So the swap can happen while the service runs: write `warmup-companion.new`, rename it over `warmup-companion`, then `systemctl --user restart warmup-companion.service`. `Restart=on-failure` does not restart a clean exit, so the explicit restart is what picks up the new binary. The helper can be `systemd-run --user` or another short-lived command. It must not be the binary that is about to be replaced.

The unit file does not need a rewrite when the path stays the same. Do not `daemon-reload` unless the unit text changed. Do not enable the unit again if it is already enabled.

If stop or rename fails, start the service again on the old binary and report the failure. Do not leave the user with no companion.

## How the user hears about it

The service already notifies through `linux_notify`. A newer build, a failed check, and a finished install can use that. The first check, the 10 minute retry, and the daily check are the cadence to mirror.

Controller Center and the tray menu are owned by the UI work. A "Check for updates" button, progress, and the `update_check` switch in those screens stay there. This plan does not add them. The setting can already be written with `warmup-companion settings`.
