use std::cell::Cell;

use crate::warmup_launch::run_boot_gamepad_loop;
use crate::App;

pub fn run() -> Result<(), String> {
    let wayland = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    if wayland.is_empty() {
        let message = "WAYLAND_DISPLAY is not set, so the companion cannot see your compositor. uwsm exports the graphical session into the user manager. On a plain Hyprland session run: dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_RUNTIME_DIR. Then: systemctl --user restart warmup-companion";
        crate::install::log_line(message);
        return Err(message.to_string());
    }
    if std::env::var_os("XDG_RUNTIME_DIR").is_none_or(|value| value.is_empty()) {
        let message = "XDG_RUNTIME_DIR is not set, so the companion cannot create its input socket. Start it inside a systemd user session, where that directory is /run/user/<uid>.";
        crate::install::log_line(message);
        return Err(message.to_string());
    }
    std::env::set_var("WARMUP_VK_SERVICE", "1");
    install_stop_signals();
    crate::install::log_line("warmup-companion service starting (SDL3 + uinput)");
    crate::linux_game_detect::start();
    crate::linux_tray::spawn();
    crate::linux_update::spawn_auto_check();
    crate::linux_vk::show_prompt_once();
    let mut app = App::default();
    app.use_real_win32 = false;
    let vk_open = Cell::new(false);
    let result = run_boot_gamepad_loop(&mut app, &vk_open, true);
    if let Some(session) = app.vk_session.take() {
        session.close();
    }
    crate::linux_hotkeys::release();
    match &result {
        Ok(()) => crate::install::log_line("service gamepad loop returned"),
        Err(error) => crate::install::log_line(&format!("service gamepad loop failed: {error}")),
    }
    result
}

fn install_stop_signals() {
    let mut mask = nix::sys::signal::SigSet::empty();
    mask.add(nix::sys::signal::Signal::SIGTERM);
    mask.add(nix::sys::signal::Signal::SIGINT);
    if let Err(error) = mask.thread_block() {
        crate::install::log_line(&format!("could not block stop signals: {error}"));
        return;
    }
    std::thread::Builder::new()
        .name("warmup-signals".into())
        .spawn(move || {
            while let Ok(signal) = mask.wait() {
                if matches!(
                    signal,
                    nix::sys::signal::Signal::SIGTERM | nix::sys::signal::Signal::SIGINT
                ) {
                    crate::install::log_line("service stop requested");
                    crate::gamepad::request_stop();
                    break;
                }
            }
        })
        .ok();
}
