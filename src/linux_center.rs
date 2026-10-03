use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;

use crate::center_rows::{RowKind, TABS};

struct Field {
    checked: bool,
    slider: i32,
    words: String,
}

struct CenterApp {
    tab: usize,
    fields: Vec<Field>,
    confirm_signin: bool,
    signin_confirmed: bool,
    close: bool,
}

pub fn show() {
    static OPEN: AtomicBool = AtomicBool::new(false);
    if OPEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-controller-center".into())
        .spawn(|| {
            let result = show_blocking();
            OPEN.store(false, Ordering::SeqCst);
            if let Err(error) = result {
                crate::install::log_line(&format!("controller center: {error}"));
            }
        });
}

pub fn show_blocking() -> Result<(), String> {
    eframe::run_native(
        "Controller Center",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([760.0, 640.0]),
            ..Default::default()
        },
        Box::new(|_cc| Ok(Box::new(CenterApp::load()))),
    )
    .map_err(|error| error.to_string())
}

impl CenterApp {
    fn load() -> Self {
        let fields = crate::center_rows::ROWS.iter().map(read_field).collect();
        Self {
            tab: 0,
            fields,
            confirm_signin: false,
            signin_confirmed: false,
            close: false,
        }
    }

    fn apply(&mut self) {
        let saved: Vec<Field> = crate::center_rows::ROWS.iter().map(read_field).collect();
        for (index, row) in crate::center_rows::ROWS.iter().enumerate() {
            let next = &self.fields[index];
            let prev = &saved[index];
            match row.kind {
                RowKind::Check {
                    key,
                    on_val,
                    off_val,
                } => {
                    if next.checked != prev.checked {
                        if key == "run_mode" && next.checked && !self.signin_confirmed {
                            self.confirm_signin = true;
                            return;
                        }
                        set(key, if next.checked { on_val } else { off_val });
                    }
                }
                RowKind::PausePoll => {
                    if next.checked != prev.checked {
                        crate::gamepad_backend::set_userland_poll_paused(next.checked);
                        let _ = crate::config::write_userland_poll_paused(next.checked);
                    }
                }
                RowKind::Slider {
                    key, div, decimals, ..
                } => {
                    if next.slider != prev.slider {
                        set(
                            key,
                            &format!("{:.*}", decimals, next.slider as f32 / div),
                        );
                    }
                }
                RowKind::Vocabulary => {
                    let staged = staged_vocabulary(next.checked, &next.words);
                    if staged != crate::config::vocabulary() {
                        set(
                            "vocabulary",
                            &crate::config::format_vocabulary(&staged),
                        );
                    }
                }
            }
        }
        self.confirm_signin = false;
    }
}

impl eframe::App for CenterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Controller Center");
            ui.horizontal(|ui| {
                for (index, name) in TABS.iter().enumerate() {
                    if ui.selectable_label(self.tab == index, *name).clicked() {
                        self.tab = index;
                    }
                }
            });
            ui.separator();
            if self.tab == 3 {
                ui.label(format!(
                    "Controller: {}",
                    crate::pipe_server::connection_name()
                ));
                ui.label("Sticks, clicks, and Enter are injected while warmUP is closed.");
            } else {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (index, row) in crate::center_rows::ROWS.iter().enumerate() {
                        if row.tab != self.tab {
                            continue;
                        }
                        let field = &mut self.fields[index];
                        match row.kind {
                            RowKind::Check { .. } | RowKind::PausePoll => {
                                ui.checkbox(&mut field.checked, row.label);
                            }
                            RowKind::Slider { min, max, div, decimals, .. } => {
                                ui.label(row.label);
                                ui.add(
                                    egui::Slider::new(&mut field.slider, min..=max)
                                        .custom_formatter(move |value, _| {
                                            format!("{:.*}", decimals, value as f32 / div)
                                        }),
                                );
                            }
                            RowKind::Vocabulary => {
                                ui.checkbox(&mut field.checked, row.label);
                                ui.label("Extra words, comma separated");
                                ui.text_edit_singleline(&mut field.words);
                            }
                        }
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Ok").clicked() {
                    self.apply();
                    if !self.confirm_signin {
                        self.close = true;
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.close = true;
                }
                if ui.button("Apply").clicked() {
                    self.apply();
                }
                if ui.button("Edit settings file").clicked() {
                    if let Some(path) = crate::config::ensure_settings_file() {
                        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
                    }
                }
            });
        });
        if self.confirm_signin {
            egui::Window::new("Sign-in screen only").show(ctx, |ui| {
                ui.label(
                    "After you next sign in or unlock, the companion sleeps on the desktop: no controller input and no tray icon until it is woken.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Continue").clicked() {
                        self.confirm_signin = false;
                        self.signin_confirmed = true;
                        self.apply();
                    }
                    if ui.button("Go back").clicked() {
                        self.confirm_signin = false;
                        for (index, row) in crate::center_rows::ROWS.iter().enumerate() {
                            if matches!(row.kind, RowKind::Check { key: "run_mode", .. }) {
                                self.fields[index].checked = false;
                            }
                        }
                    }
                });
            });
        }
        if self.close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn read_field(row: &crate::center_rows::Row) -> Field {
    let settings = crate::config::gamepad_settings();
    match row.kind {
        RowKind::Check { key, .. } => Field {
            checked: match key {
                "cursor_enabled" => settings.cursor_enabled,
                "sleep_on_game" => settings.sleep_on_game,
                "auto_stop_on_game" => settings.auto_stop_on_game,
                "signin_hints" => settings.signin_hints,
                "guide_launch" => settings.guide_launch,
                "natural_scroll" => settings.natural_scroll,
                "vk_mode" => {
                    crate::config::vk_layout_mode() == crate::config::VkLayoutMode::Floating
                }
                "vk_bar_scale" => crate::config::vk_bar_scale() < 1.0,
                "run_mode" => crate::config::run_mode() == crate::config::RunMode::SignIn,
                _ => false,
            },
            slider: 0,
            words: String::new(),
        },
        RowKind::PausePoll => Field {
            checked: crate::gamepad_backend::userland_poll_paused(),
            slider: 0,
            words: String::new(),
        },
        RowKind::Slider { key, div, .. } => {
            let value = match key {
                "cursor_speed" => settings.cursor_speed,
                "cursor_accel" => settings.cursor_accel,
                "cursor_deadzone" => settings.cursor_deadzone,
                "cursor_smoothing" => settings.cursor_smoothing,
                "scroll_speed" => settings.scroll_speed,
                "scroll_accel" => settings.scroll_accel,
                _ => 0.0,
            };
            Field {
                checked: false,
                slider: (value * div).round() as i32,
                words: String::new(),
            }
        }
        RowKind::Vocabulary => {
            let vocab = crate::config::vocabulary();
            Field {
                checked: vocab.coding,
                slider: 0,
                words: vocab.words.join(", "),
            }
        }
    }
}

fn set(key: &str, value: &str) {
    if let Err(error) = crate::config::set_gamepad_setting(key, value) {
        crate::install::log_line(&format!("controller center: set {key}={value}: {error}"));
    }
}

fn staged_vocabulary(coding: bool, words: &str) -> crate::config::Vocabulary {
    let cleaned: String = words
        .chars()
        .map(|ch| if ch == '=' || ch.is_control() { ' ' } else { ch })
        .collect();
    let mut vocab = crate::config::parse_vocabulary_list(&cleaned);
    vocab.coding = coding;
    vocab
}
