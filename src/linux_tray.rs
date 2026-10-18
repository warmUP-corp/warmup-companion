use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip, Tray};

use crate::win::tray_menu::{Action, Entry, Placement, Reply, Spec, POINT, ROOT_W, SUB_W};

const BUILD_INFO: &str = concat!(
    "v",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("WARMUP_BUILD_CHECKSUM"),
    ")"
);

static STARTED: AtomicBool = AtomicBool::new(false);
static HANDLE: Mutex<Option<ksni::Handle<CompanionTray>>> = Mutex::new(None);

struct CompanionTray;

pub fn spawn() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-tray".into())
        .spawn(|| {
            crate::linux_notify::show_welcome_if_first_run();
            let service = ksni::TrayService::new(CompanionTray);
            if let Ok(mut handle) = HANDLE.lock() {
                *handle = Some(service.handle());
            }
            if let Err(error) = service.run() {
                crate::install::log_line(&format!("tray stopped: {error}"));
            }
        });
}

pub(crate) fn refresh() {
    if let Some(handle) = HANDLE.lock().ok().and_then(|h| h.clone()) {
        handle.update(|_| {});
    }
}

impl Tray for CompanionTray {
    fn activate(&mut self, x: i32, y: i32) {
        let at = if x <= 0 && y <= 0 {
            POINT { x: i32::MAX / 2, y: 0 }
        } else {
            POINT { x, y }
        };
        crate::win::tray_menu::open_with(menu_spec, at);
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
            action("Control Center…", |_| crate::win::controller_center::show()),
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
                            check(
                                "Modern",
                                crate::config::vk_style() == crate::config::VkStyle::Modern,
                                |_| set_style("modern"),
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
            submenu("Updates", update_items(settings.update_check)),
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

const CMD_PAUSE: usize = 1;
const CMD_CURSOR: usize = 2;
const CMD_CENTER: usize = 3;
const CMD_SIGNIN_HINTS: usize = 4;
const CMD_COMPACT: usize = 5;
const CMD_FLOATING: usize = 6;
const CMD_STYLE_NORMAL: usize = 7;
const CMD_STYLE_MONO: usize = 8;
const CMD_STYLE_MODERN: usize = 9;
const CMD_SLEEP_ON_GAME: usize = 10;
const CMD_AUTOSTOP_ON_GAME: usize = 11;
const CMD_VOICE: usize = 12;
const CMD_UPDATE: usize = 13;
const CMD_UPDATE_NOTES: usize = 14;
const CMD_UPDATE_AUTO: usize = 15;
const CMD_OPEN_LOG: usize = 16;
const CMD_DIAGNOSTICS: usize = 17;
const CMD_EDIT_SETTINGS: usize = 18;
const CMD_PRIVACY: usize = 19;
const CMD_UNINSTALL: usize = 20;
const CMD_EXIT: usize = 21;

fn menu_entries() -> Vec<Entry> {
    let gs = crate::config::gamepad_settings();
    let style = crate::config::vk_style();
    let available = crate::linux_update::available();
    let mut updates = match &available {
        Some((v, _)) => vec![
            Entry::command(format!("Install update v{v}…"), CMD_UPDATE),
            Entry::command("What's new", CMD_UPDATE_NOTES),
        ],
        None => vec![Entry::command("Check for updates", CMD_UPDATE)],
    };
    updates.push(Entry::Separator);
    updates.push(Entry::toggle("Check automatically", CMD_UPDATE_AUTO, gs.update_check));
    vec![
        Entry::Header,
        Entry::Separator,
        Entry::toggle(
            "Pause gamepad input",
            CMD_PAUSE,
            crate::gamepad_backend::userland_poll_paused(),
        ),
        Entry::toggle("Gamepad cursor", CMD_CURSOR, gs.cursor_enabled),
        Entry::command("Control Center…", CMD_CENTER).with_accel("Select+Start"),
        Entry::toggle("Sign-in controller hints", CMD_SIGNIN_HINTS, gs.signin_hints),
        Entry::submenu(
            "Keyboard",
            vec![
                Entry::toggle(
                    "Compact size",
                    CMD_COMPACT,
                    crate::config::vk_bar_scale() < 1.0,
                ),
                Entry::toggle(
                    "Floating layout",
                    CMD_FLOATING,
                    crate::config::vk_layout_mode() == crate::config::VkLayoutMode::Floating,
                ),
                Entry::submenu(
                    "Keyboard style",
                    vec![
                        Entry::radio(
                            "Normal",
                            CMD_STYLE_NORMAL,
                            style == crate::config::VkStyle::Normal,
                        ),
                        Entry::radio("Mono", CMD_STYLE_MONO, style == crate::config::VkStyle::Mono),
                        Entry::radio(
                            "Modern",
                            CMD_STYLE_MODERN,
                            style == crate::config::VkStyle::Modern,
                        ),
                    ],
                ),
            ],
        ),
        Entry::submenu(
            "Game detection",
            vec![
                Entry::toggle(
                    "Pause input while a game is running",
                    CMD_SLEEP_ON_GAME,
                    gs.sleep_on_game,
                ),
                Entry::toggle(
                    "Guide-only input while a game is running",
                    CMD_AUTOSTOP_ON_GAME,
                    gs.auto_stop_on_game,
                ),
            ],
        ),
        Entry::toggle("Voice typing", CMD_VOICE, gs.voice_enabled),
        Entry::submenu("Updates", updates).with_dot(available.is_some()),
        Entry::submenu(
            "Diagnostics",
            vec![
                Entry::command("Open service log", CMD_OPEN_LOG),
                Entry::command("Run diagnostics", CMD_DIAGNOSTICS),
                Entry::command("Edit settings file…", CMD_EDIT_SETTINGS),
                Entry::command("Privacy & trust model", CMD_PRIVACY),
            ],
        ),
        Entry::Separator,
        Entry::command("Uninstall Warmup Companion…", CMD_UNINSTALL),
        Entry::Separator,
        Entry::command("Exit", CMD_EXIT),
    ]
}

fn run_command(cmd: usize) {
    let gs = crate::config::gamepad_settings();
    match cmd {
        CMD_PAUSE => toggle_pause(),
        CMD_CURSOR => toggle_bool("cursor_enabled", gs.cursor_enabled),
        CMD_CENTER => crate::win::controller_center::show(),
        CMD_SIGNIN_HINTS => toggle_bool("signin_hints", gs.signin_hints),
        CMD_COMPACT => toggle_compact(),
        CMD_FLOATING => toggle_vk_mode(),
        CMD_STYLE_NORMAL => set_style("normal"),
        CMD_STYLE_MONO => set_style("mono"),
        CMD_STYLE_MODERN => set_style("modern"),
        CMD_SLEEP_ON_GAME => toggle_bool("sleep_on_game", gs.sleep_on_game),
        CMD_AUTOSTOP_ON_GAME => toggle_bool("auto_stop_on_game", gs.auto_stop_on_game),
        CMD_VOICE => toggle_bool("voice_enabled", gs.voice_enabled),
        CMD_UPDATE if crate::linux_update::available().is_some() => {
            crate::linux_update::start_update()
        }
        CMD_UPDATE => crate::linux_update::check_now(),
        CMD_UPDATE_NOTES => {
            if let Some((_, page)) = crate::linux_update::available() {
                let _ = std::process::Command::new("xdg-open").arg(page).spawn();
            }
        }
        CMD_UPDATE_AUTO => toggle_bool("update_check", gs.update_check),
        CMD_OPEN_LOG => open_path(&crate::paths::log_file()),
        CMD_DIAGNOSTICS => run_diagnostics(),
        CMD_EDIT_SETTINGS => {
            if let Some(path) = crate::config::ensure_settings_file() {
                open_path(&path);
            }
        }
        CMD_PRIVACY => show_privacy(),
        CMD_UNINSTALL => uninstall(),
        CMD_EXIT => crate::gamepad::request_stop(),
        _ => {}
    }
    refresh();
}

fn menu_spec(at: POINT) -> Spec {
    Spec {
        root: menu_entries(),
        root_w: ROOT_W,
        sub_w: SUB_W,
        title: "warmUP Companion".to_string(),
        subtitle: BUILD_INFO.to_string(),
        zoom: 1.0,
        placement: Placement::Anchor(at),
        handler: Box::new(|action| match action {
            Action::Run(cmd) => Reply::Close(Some(Box::new(move || run_command(cmd)))),
            _ => Reply::Stay,
        }),
        refresh: None,
        on_close: None,
    }
}

fn update_items(auto: bool) -> Vec<MenuItem<CompanionTray>> {
    let mut items = match crate::linux_update::available() {
        Some((version, page)) => vec![
            action(&format!("Install update v{version}…"), |_| {
                crate::linux_update::start_update()
            }),
            action("What's new", move |_| {
                let _ = std::process::Command::new("xdg-open").arg(&page).spawn();
            }),
        ],
        None => vec![action("Check for updates", |_| {
            crate::linux_update::check_now()
        })],
    };
    items.push(MenuItem::Separator);
    items.push(check("Check automatically", auto, |_| {
        toggle_bool("update_check", crate::config::gamepad_settings().update_check);
    }));
    items
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
