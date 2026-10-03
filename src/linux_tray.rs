use std::sync::atomic::{AtomicBool, Ordering};

use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip, Tray};

const BUILD_INFO: &str = concat!(
    "v",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("WARMUP_BUILD_CHECKSUM"),
    ")"
);

static STARTED: AtomicBool = AtomicBool::new(false);

struct CompanionTray;

pub fn spawn() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-tray".into())
        .spawn(|| {
            crate::linux_notify::show_welcome_if_first_run();
            if let Err(error) = ksni::TrayService::new(CompanionTray).run() {
                crate::install::log_line(&format!("tray stopped: {error}"));
            }
        });
}

impl Tray for CompanionTray {
    fn activate(&mut self, _x: i32, _y: i32) {
        crate::linux_center::show();
    }

    fn id(&self) -> String {
        "warmup-companion".into()
    }

    fn title(&self) -> String {
        "Warmup Companion".into()
    }

    fn icon_name(&self) -> String {
        "input-gaming".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![Icon {
            width: 22,
            height: 22,
            data: pixmap(),
        }]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: format!("Warmup Companion {BUILD_INFO}"),
            description: "Controller pointer, scroll, click, and Enter".into(),
            icon_name: "input-gaming".into(),
            icon_pixmap: self.icon_pixmap(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let settings = crate::config::gamepad_settings();
        let paused = crate::gamepad_backend::userland_poll_paused();
        let mut items = vec![
            disabled("Warmup Companion"),
            disabled(BUILD_INFO),
            MenuItem::Separator,
            check("Pause gamepad input", paused, |_| toggle_pause()),
            check("Gamepad cursor", settings.cursor_enabled, |_| {
                toggle_bool("cursor_enabled", crate::config::gamepad_settings().cursor_enabled);
            }),
            action("Controller Center...", |_| crate::linux_center::show()),
            action("Show controller tips", |_| {
                let _ = crate::linux_notify::notify(
                    "Warmup Companion",
                    "Controller tips are shown with the on-screen keyboard, which is not part of this Linux build.",
                );
            }),
            check("Sign-in controller hints", settings.signin_hints, |_| {
                toggle_bool("signin_hints", crate::config::gamepad_settings().signin_hints);
            }),
            submenu(
                "Keyboard",
                vec![
                    check(
                        "Compact size",
                        crate::config::vk_bar_scale() < 1.0,
                        |_| toggle_compact(),
                    ),
                    check(
                        "Floating layout",
                        crate::config::vk_layout_mode() == crate::config::VkLayoutMode::Floating,
                        |_| toggle_vk_mode(),
                    ),
                    submenu(
                        "Keyboard style",
                        vec![
                            check(
                                "Normal",
                                crate::config::vk_style() == crate::config::VkStyle::Normal,
                                |_| set_style("normal"),
                            ),
                            check(
                                "Mono",
                                crate::config::vk_style() == crate::config::VkStyle::Mono,
                                |_| set_style("mono"),
                            ),
                        ],
                    ),
                ],
            ),
            submenu(
                "Game detection",
                vec![
                    check(
                        "Pause input while a game is running",
                        settings.sleep_on_game,
                        |_| {
                            toggle_bool(
                                "sleep_on_game",
                                crate::config::gamepad_settings().sleep_on_game,
                            );
                        },
                    ),
                    check(
                        "Guide-only input while a game is running",
                        settings.auto_stop_on_game,
                        |_| {
                            toggle_bool(
                                "auto_stop_on_game",
                                crate::config::gamepad_settings().auto_stop_on_game,
                            );
                        },
                    ),
                ],
            ),
            submenu(
                "Diagnostics",
                vec![
                    action("Open service log", |_| open_path(&crate::paths::log_file())),
                    action("Run diagnostics", |_| run_diagnostics()),
                    action("Edit settings file…", |_| {
                        if let Some(path) = crate::config::ensure_settings_file() {
                            open_path(&path);
                        }
                    }),
                    action("Privacy & trust model", |_| show_privacy()),
                ],
            ),
            MenuItem::Separator,
            action("Uninstall", |_| uninstall()),
            MenuItem::Separator,
            action("Exit", |_| crate::gamepad::request_stop()),
        ];
        items
    }
}

fn action(
    label: &str,
    activate: impl Fn(&mut CompanionTray) + Send + 'static,
) -> MenuItem<CompanionTray> {
    StandardItem {
        label: label.into(),
        activate: Box::new(activate),
        ..Default::default()
    }
    .into()
}

fn check(
    label: &str,
    checked: bool,
    activate: impl Fn(&mut CompanionTray) + Send + 'static,
) -> MenuItem<CompanionTray> {
    CheckmarkItem {
        label: label.into(),
        checked,
        activate: Box::new(activate),
        ..Default::default()
    }
    .into()
}

fn disabled(label: &str) -> MenuItem<CompanionTray> {
    StandardItem {
        label: label.into(),
        enabled: false,
        ..Default::default()
    }
    .into()
}

fn submenu(label: &str, submenu: Vec<MenuItem<CompanionTray>>) -> MenuItem<CompanionTray> {
    SubMenu {
        label: label.into(),
        submenu,
        ..Default::default()
    }
    .into()
}

fn toggle_pause() {
    let paused = !crate::gamepad_backend::userland_poll_paused();
    crate::gamepad_backend::set_userland_poll_paused(paused);
    let _ = crate::config::write_userland_poll_paused(paused);
}

fn toggle_bool(key: &str, current: bool) {
    let next = if current { "false" } else { "true" };
    if let Err(error) = crate::config::set_gamepad_setting(key, next) {
        crate::install::log_line(&format!("tray: set {key} failed: {error}"));
    }
}

fn toggle_compact() {
    let next = if crate::config::vk_bar_scale() < 1.0 {
        "1.0".to_string()
    } else {
        crate::config::COMPACT_BAR_SCALE.to_string()
    };
    if let Err(error) = crate::config::set_gamepad_setting("vk_bar_scale", &next) {
        crate::install::log_line(&format!("tray: set vk_bar_scale failed: {error}"));
    }
}

fn toggle_vk_mode() {
    let floating = crate::config::vk_layout_mode() == crate::config::VkLayoutMode::Floating;
    let next = if floating { "docked" } else { "floating" };
    if let Err(error) = crate::config::set_gamepad_setting("vk_mode", next) {
        crate::install::log_line(&format!("tray: set vk_mode failed: {error}"));
    }
}

fn set_style(style: &str) {
    if let Err(error) = crate::config::set_gamepad_setting("vk_style", style) {
        crate::install::log_line(&format!("tray: set vk_style failed: {error}"));
    }
}

fn open_path(path: &std::path::Path) {
    match std::process::Command::new("xdg-open").arg(path).spawn() {
        Ok(_) => {}
        Err(error) => {
            crate::install::log_line(&format!("open {}: {error}", path.display()));
        }
    }
}

fn run_diagnostics() {
    let _ = std::thread::Builder::new()
        .name("warmup-diagnostics".into())
        .spawn(|| match crate::linux_diag::write_report() {
            Ok(path) => {
                crate::install::log_line(&format!("diagnostics: {}", path.display()));
                let _ = crate::linux_notify::notify(
                    "Warmup Companion diagnostics",
                    &format!("Saved {}", path.display()),
                );
            }
            Err(error) => {
                crate::install::log_line(&format!("diagnostics failed: {error}"));
            }
        });
}

fn show_privacy() {
    let log = crate::paths::log_file();
    let _ = crate::linux_notify::notify(
        "Warmup Companion privacy",
        &format!(
            "Warmup Companion does not read host app text. Sentry is disabled unless WARMUP_SENTRY_DSN is set. Service log: {}",
            log.display()
        ),
    );
}

fn uninstall() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe).arg("uninstall").spawn();
}

fn pixmap() -> Vec<u8> {
    let mut data = Vec::with_capacity(22 * 22 * 4);
    for y in 0..22 {
        for x in 0..22 {
            let edge = x < 2 || y < 2 || x > 19 || y > 19;
            let (a, r, g, b) = if edge {
                (255, 230, 236, 245)
            } else {
                (255, 36, 99, 168)
            };
            data.extend_from_slice(&[a, r, g, b]);
        }
    }
    data
}
