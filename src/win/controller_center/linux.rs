use std::path::{Path, PathBuf};

use super::*;
use crate::linux_panel::{self, Flow, Key, PanelApp, PanelThread, Spec};

pub(super) use crate::vk_theme::current_theme;
pub(super) use vk_renderer::VkPalette;

pub(super) type HWND = ();

pub(super) const TYPE_PLACEHOLDER: &str = "Program name, for example firefox";
pub(super) const TYPE_CAPTION: &str = "Opens the keyboard \u{00B7} for example firefox";

pub(super) struct VirtualKey(pub(super) u16);

pub(super) const VK_SHIFT: VirtualKey = VirtualKey(0x10);
pub(super) const VK_CONTROL: VirtualKey = VirtualKey(0x11);
pub(super) const VK_TAB: VirtualKey = VirtualKey(0x09);
pub(super) const VK_RETURN: VirtualKey = VirtualKey(0x0D);
pub(super) const VK_ESCAPE: VirtualKey = VirtualKey(0x1B);
pub(super) const VK_SPACE: VirtualKey = VirtualKey(0x20);
pub(super) const VK_LEFT: VirtualKey = VirtualKey(0x25);
pub(super) const VK_UP: VirtualKey = VirtualKey(0x26);
pub(super) const VK_RIGHT: VirtualKey = VirtualKey(0x27);
pub(super) const VK_DOWN: VirtualKey = VirtualKey(0x28);
pub(super) const VK_DELETE: VirtualKey = VirtualKey(0x2E);

#[allow(non_snake_case)]
pub(super) unsafe fn GetKeyState(vk: i32) -> i16 {
    let m = linux_panel::mods();
    let down = match vk {
        0x10 => m.shift,
        0x11 => m.ctrl,
        0x12 => m.alt,
        0x5B | 0x5C => m.logo,
        _ => false,
    };
    if down {
        -1
    } else {
        0
    }
}

pub(super) mod vk_renderer {
    pub(crate) use crate::vk_paint::{is_playstation_label, style_palette, VkPalette};
}

static THREAD: OnceLock<Mutex<Option<PanelThread>>> = OnceLock::new();

pub fn show() {
    let slot = THREAD.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    apps::prewarm();
    if guard.is_none() {
        let spec = Spec {
            thread: "warmup-controller-center",
            title: "Control Center",
            app_id: "warmup-companion",
        };
        match linux_panel::spawn(spec, Box::new(CenterPanel)) {
            Ok(t) => *guard = Some(t),
            Err(e) => {
                crate::install::log_line(&format!("control center: spawn failed: {e}"));
                return;
            }
        }
    }
    if let Some(t) = guard.as_ref() {
        let _ = take_pad_edges();
        OWNS_PAD.store(true, Ordering::SeqCst);
        if t.show().is_err() {
            OWNS_PAD.store(false, Ordering::SeqCst);
        }
    }
}

pub(super) fn hide() {
    OWNS_PAD.store(false, Ordering::SeqCst);
    if let Some(t) = THREAD
        .get()
        .and_then(|s| s.lock().ok())
        .as_ref()
        .and_then(|g| g.as_ref())
    {
        let _ = t.hide();
    }
}

const MIC_OPTIONS: usize = 4;
const MIC_LABEL_CHARS: usize = 17;

pub(super) fn center_items() -> Vec<Item> {
    let mut items: Vec<Item> = items()
        .into_iter()
        .filter(|it| !matches!(it.kind, Kind::Check { key: "run_mode", .. }))
        .collect();
    #[cfg(feature = "speech")]
    {
        items.push(item(
            5,
            "Microphone",
            Kind::Choice {
                key: "voice_mic",
                options: mic_options(),
            },
        ));
    }
    items
}

#[cfg(feature = "speech")]
fn mic_options() -> &'static [(&'static str, &'static str)] {
    static OPTIONS: Mutex<Vec<(Vec<crate::linux_speech::Mic>, &'static [(&'static str, &'static str)])>> =
        Mutex::new(Vec::new());
    let mut mics = crate::linux_speech::list_mics();
    if let Some(chosen) = crate::linux_speech::mic_choice() {
        if let Some(i) = mics.iter().position(|m| m.name == chosen) {
            let mic = mics.remove(i);
            mics.insert(0, mic);
        }
    }
    mics.truncate(MIC_OPTIONS - 1);
    let Ok(mut cache) = OPTIONS.lock() else {
        return &[("Default", "")];
    };
    if let Some((_, options)) = cache.iter().find(|(known, _)| *known == mics) {
        return options;
    }
    let mut options: Vec<(&'static str, &'static str)> = vec![("Default", "")];
    let firsts: Vec<String> = mics.iter().map(|m| short_label(&m.label, false)).collect();
    for (i, mic) in mics.iter().enumerate() {
        let clash = firsts.iter().filter(|f| **f == firsts[i]).count() > 1;
        let label = short_label(&mic.label, clash);
        options.push((
            Box::leak(label.into_boxed_str()),
            Box::leak(mic.name.clone().into_boxed_str()),
        ));
    }
    let options: &'static [(&'static str, &'static str)] = Box::leak(options.into_boxed_slice());
    cache.push((mics, options));
    options
}

#[cfg(feature = "speech")]
fn short_label(label: &str, with_tail: bool) -> String {
    let base = label.split(" (").next().unwrap_or(label);
    let words: Vec<&str> = base.split_whitespace().collect();
    let mut short = words.first().copied().unwrap_or(label).to_string();
    if with_tail && words.len() > 1 {
        let tail = words[words.len().saturating_sub(2).max(1)..].join(" ");
        short = format!("{short} {tail}");
    }
    if short.chars().count() <= MIC_LABEL_CHARS {
        return short;
    }
    let cut: String = short.chars().take(MIC_LABEL_CHARS - 1).collect();
    format!("{}\u{2026}", cut.trim_end())
}

pub(super) fn mic_value() -> String {
    #[cfg(feature = "speech")]
    {
        crate::linux_speech::mic_choice().unwrap_or_default()
    }
    #[cfg(not(feature = "speech"))]
    {
        String::new()
    }
}

pub(super) fn set_mic(name: &str) {
    #[cfg(feature = "speech")]
    crate::linux_speech::set_mic_choice(name);
    #[cfg(not(feature = "speech"))]
    let _ = name;
}

pub(super) fn close_window(_ui: &Ui) {}

pub(super) fn set_capture(_hwnd: HWND) {}

pub(super) fn get_launcher() {}

pub(super) fn promo_art(_scale: f32) -> Option<(i32, i32, Vec<u8>)> {
    None
}

pub(super) unsafe fn draw_promo_art(g: &mut Gfx, w: i32, h: i32, px: &[u8], art: Rect) {
    let rgba: Vec<u8> = px
        .chunks_exact(4)
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    g.set_image("promo", w as u32, h as u32, &rgba);
    g.draw_image("promo", art, 1.0, false);
}

pub(super) unsafe fn confirm_signin_only(_owner: HWND) -> bool {
    false
}

pub(super) fn run_voice_download(_id: &'static str) -> bool {
    false
}

pub(super) fn program_name(name: &str) -> String {
    name.to_string()
}

fn documents_dir() -> PathBuf {
    let named = std::process::Command::new("xdg-user-dir")
        .arg("DOCUMENTS")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    named.unwrap_or_else(|| {
        let docs = home.join("Documents");
        if docs.is_dir() {
            docs
        } else {
            home
        }
    })
}

fn newest_export(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("warmup-companion-settings-") && name.ends_with(".ini")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(at, _)| *at)
        .map(|(_, path)| path)
}

pub(super) unsafe fn pick_file(
    _owner: HWND,
    save: bool,
    _dirty: bool,
) -> Result<Option<PathBuf>, String> {
    let dir = documents_dir();
    if save {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let (y, mo, d, h, mi, sec) = crate::time_util::local_now();
        let name = export_file_name(y, mo, d).replace(".ini", &format!("-{h:02}{mi:02}{sec:02}.ini"));
        return Ok(Some(dir.join(name)));
    }
    newest_export(&dir)
        .map(Some)
        .ok_or_else(|| format!("no exported settings in {}", dir.display()))
}

fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut dirs = vec![std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))];
    let shared = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(shared.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs
}

fn desktop_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return Some(v.trim());
            }
        }
    }
    None
}

fn program_of(exec: &str) -> Option<String> {
    let mut parts = exec.split_whitespace().map(|p| p.trim_matches('"'));
    let mut program = parts.next()?;
    if program == "env" {
        program = parts.find(|p| !p.contains('='))?;
    }
    let base = program.rsplit('/').next()?.to_string();
    (!base.is_empty()).then_some(base)
}

pub(super) fn installed_apps() -> Vec<(String, String)> {
    const WRAPPERS: [&str; 6] = ["flatpak", "snap", "sh", "bash", "gtk-launch", "gio"];
    let mut seen_ids: Vec<String> = Vec::new();
    let mut out: Vec<(String, String)> = Vec::new();
    for dir in data_dirs() {
        let Ok(entries) = std::fs::read_dir(dir.join("applications")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            if seen_ids.contains(&id) {
                continue;
            }
            seen_ids.push(id);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let flag = |k: &str| desktop_value(&text, k).is_some_and(|v| v == "true");
            if desktop_value(&text, "Type") != Some("Application")
                || flag("NoDisplay")
                || flag("Hidden")
            {
                continue;
            }
            let (Some(name), Some(exec)) = (desktop_value(&text, "Name"), desktop_value(&text, "Exec"))
            else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if ["uninstall", "readme", "help", "documentation", "website"]
                .iter()
                .any(|w| lower.contains(w))
            {
                continue;
            }
            let Some(mut program) = program_of(exec) else {
                continue;
            };
            if WRAPPERS.contains(&program.as_str()) {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                program = desktop_value(&text, "StartupWMClass")
                    .map(str::to_string)
                    .unwrap_or(stem);
            }
            if program.is_empty() || out.iter().any(|(_, e)| e.eq_ignore_ascii_case(&program)) {
                continue;
            }
            out.push((name.to_string(), program));
        }
    }
    out.sort_by_key(|(n, _)| n.to_lowercase());
    out
}

fn flow(after: After) -> Flow {
    match after {
        After::Close => Flow::Close,
        After::Stay => Flow::Paint,
    }
}

struct CenterPanel;

impl PanelApp for CenterPanel {
    fn size(&self) -> (u32, u32) {
        (WIN_W as u32, WIN_H as u32)
    }

    fn open(&mut self, scale: f32) {
        OWNS_PAD.store(true, Ordering::SeqCst);
        if UI.with(|u| u.borrow().is_some()) {
            return;
        }
        let ui = new_ui((), scale);
        UI.with(|u| *u.borrow_mut() = Some(ui));
        with_ui(|ui| {
            ui.led_base = led_values(ui);
            ui.led_previewed = ui.led_base;
        });
    }

    fn close(&mut self) {
        ui_hide();
    }

    fn rescale(&mut self, scale: f32) {
        with_ui(|ui| {
            ui.scale = scale;
            ui.promo = promo_art(scale);
        });
    }

    fn caption(&mut self, x: f32, y: f32) -> bool {
        hit_caption(x, y)
    }

    fn press(&mut self, x: f32, y: f32) -> Flow {
        let mut after = After::Stay;
        with_ui(|ui| after = click(ui, x, y));
        flow(after)
    }

    fn motion(&mut self, x: f32, _y: f32) -> Flow {
        let mut changed = false;
        with_ui(|ui| {
            let Some(i) = ui.drag else {
                return;
            };
            let Some((_, r)) = rows(ui.tab, &ui.items, ui.led_open)
                .into_iter()
                .find(|(t, _)| *t == Target::Item(i))
            else {
                return;
            };
            if let Kind::Slider { min, max, .. } = ui.items[i].kind {
                let v = slider_pos_at(r, min, max, x);
                if v != ui.items[i].value {
                    ui.items[i].value = v;
                    changed = true;
                }
            }
        });
        if changed {
            Flow::Paint
        } else {
            Flow::Idle
        }
    }

    fn release(&mut self) -> Flow {
        with_ui(|ui| ui.drag = None);
        Flow::Idle
    }

    fn wheel(&mut self, delta: i32) -> Flow {
        let mut changed = false;
        with_ui(|ui| changed = apps::wheel(ui, delta));
        if changed {
            Flow::Paint
        } else {
            Flow::Idle
        }
    }

    fn key(&mut self, key: Key) -> Flow {
        let mut result = Flow::Idle;
        with_ui(|ui| {
            if key.mods.alt {
                if let Some(vk) = key.vk.filter(|_| apps::records_keys(ui)) {
                    let _ = key_down(ui, vk);
                    result = Flow::Paint;
                }
                return;
            }
            let mut after = After::Stay;
            if let Some(vk) = key.vk {
                after = key_down(ui, vk);
            }
            if matches!(after, After::Stay) && apps::typing(ui) {
                for ch in key.text.as_deref().unwrap_or("").chars() {
                    apps::char_input(ui, ch);
                }
            }
            result = flow(after);
        });
        result
    }

    fn tick(&mut self) -> Flow {
        let mut result = Flow::Idle;
        with_ui(|ui| {
            let (dirty, after) = tick(ui);
            result = match after {
                After::Close => Flow::Close,
                After::Stay if dirty => Flow::Paint,
                After::Stay => Flow::Idle,
            };
        });
        result
    }

    fn paint(&mut self, g: &mut Gfx, scale: f32) {
        with_ui(|ui| {
            sync_led_preview(ui);
            unsafe {
                g.transform(scale, 0.0, 0.0);
                draw_window(ui, g);
            }
        });
    }
}
