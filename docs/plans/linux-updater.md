# Linux self-update

The Windows updater in `src/updater.rs` stays Windows-only. The Linux updater is `src/linux_update.rs`. Both read the release feed through `src/release_feed.rs`: the GitHub URLs, the tag and version compare, the asset lookup, the release notes, and the `.sha256` sidecar format.

## What is built

- `warmup-companion update` checks the latest release, downloads the Linux build, verifies its checksum, renames it over the installed binary, and runs `systemctl --user try-restart warmup-companion.service` while the service is running.
- `warmup-companion update --check` only reports what the latest release has.
- `warmup-companion update --dry-run` downloads and verifies, then deletes the download. Nothing is replaced and nothing restarts.
- `warmup-companion update --dir <path>` installs into another folder and never restarts the service. Use it to test.
- The service checks 15 seconds after start, retries after 10 minutes when a check fails, and checks again a day after a successful check, while `update_check` is on. A newer release with a Linux build shows a notification that says to run `warmup-companion update`. A release without a Linux build is only logged.
- `linux_update::start_update()` runs the same update on a background thread and queues the restart with `systemctl --user --no-block restart`, so it can be called from inside the service. Nothing calls it yet. The tray or Controller Center can add a button for it.
- Downloads use `curl` with HTTPS only, including redirects. The checksum is computed with `sha256sum`. Both ship with every distribution the install guide covers, so the companion needs no TLS or hash crate.

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

GitHub releases do not publish a Linux binary today. The Windows updater requires `warmup-companion-setup.exe`. The Linux updater never downloads that installer.

Until a release carries the Linux asset, `warmup-companion update` prints that the release has no Linux build and changes nothing.

The release workflow has to upload two files to the GitHub release:

| File | Contents |
| --- | --- |
| `warmup-companion-linux-x86_64` | The release binary from `cargo build --release` on Linux, uploaded as is (not archived). |
| `warmup-companion-linux-x86_64.sha256` | The SHA-256 of that file as lowercase hex, as the first word of the file. The output of `sha256sum warmup-companion-linux-x86_64` works. |

The suffix is Rust's `std::env::consts::ARCH`. An ARM build would be `warmup-companion-linux-aarch64` with its own `.sha256`. The updater reads only the asset for the architecture it runs on.

Both URLs stay restricted to `https://github.com/warmUP-corp/warmup-companion/releases/download/`. Verify SHA-256 before the new bytes replace anything. A mismatch deletes the download and leaves the installed binary as it is.

## Replacing a running binary

The service is the process executing the installed binary. Opening that file for writing fails with `ETXTBSY`, but `rename()` over it succeeds: the running process keeps the old inode until it exits.

So the service does not have to stop first:

1. Download to `warmup-companion.new` in the same directory, so the rename stays on one filesystem.
2. Verify the checksum there and make it executable. A mismatch deletes the download.
3. Keep the current binary as `warmup-companion.old` (a hard link, or a copy if linking fails). An older `.old` is replaced. If this fails, the update stops before anything is swapped.
4. Rename `warmup-companion.new` onto `warmup-companion`.
5. `systemctl --user try-restart warmup-companion.service`, only while the service is active. `Restart=on-failure` does not restart a clean exit, so the explicit restart is what picks up the new binary. `try-restart` never starts a service the user stopped; the update then takes effect the next time it starts, and the CLI says so.

From the CLI the restart waits for systemd. From inside the service it is queued with `--no-block`: the restart stops the service's own cgroup, and a queued job finishes even after the caller is gone. No separate helper process is needed.

If the download, the checksum, or the rename fails, the old binary stays in place and the running service is not touched. If the restart fails after the rename, the new binary is already installed and the service keeps running the old one until it next starts. From the CLI, the updater also waits three seconds after the restart and fails if the service is not `active`, which catches a new build that crashes on start. Every failure after the rename, and every successful CLI update, prints how to roll back:

```sh
mv ~/.local/share/warmup-companion/bin/warmup-companion.old ~/.local/share/warmup-companion/bin/warmup-companion && systemctl --user try-restart warmup-companion.service
```

The restart is skipped when the service is not running, when the user unit file does not exist, and always with `--dir`. The unit file does not need a rewrite when the path stays the same, so the updater never runs `daemon-reload` or `enable`.

## How the user hears about it

The service notifies through `linux_notify` when a newer release has a Linux build. `start_update()` also notifies when the release has no Linux build, when the installed version is current, and when the update fails.

Controller Center and the tray menu are owned by the UI work. A "Check for updates" button, progress, and the `update_check` switch in those screens stay there. The setting can already be written with `warmup-companion settings`.
