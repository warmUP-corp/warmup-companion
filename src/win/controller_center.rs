//! Native Controller Center window, rendered through the existing D3D/D2D path.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN,
    VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, KillTimer, SetForegroundWindow, SetTimer,
    ShowWindow, CW_USEDEFAULT, HMENU, SW_HIDE, SW_SHOWNORMAL, WM_CHAR, WM_CLOSE, WM_DESTROY,
    WM_KEYDOWN, WM_LBUTTONUP, WM_PAINT, WM_SIZE, WM_SYSKEYDOWN, WM_TIMER,
    WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW,
};

use crate::controller_shortcuts::{
    self, ControllerAction, ControllerChord, DesktopActionKind, Shortcut, MAPPABLE_BUTTONS,
};
use crate::gamepad_backend::{BatteryFrame, Button, ButtonChange};

use super::desktop_window::{self, DesktopApp, DesktopWindowThread};
use super::vk_renderer::{
    controller_center_hit, ControllerCenterBinding, ControllerCenterFrame, ControllerCenterHit,
    VkRenderer,
};

const WINDOW_CLASS: PCWSTR = w!("WarmupControllerCenterWindow");
const WINDOW_BG: u32 = 0x00101010;
const REPAINT_TIMER_ID: usize = 27;
const REPAINT_TIMER_MS: u32 = 33;

#[derive(Clone, Default)]
struct ControllerSnapshot {
    connected: bool,
    name: String,
    input: String,
    axes: (f32, f32, f32, f32),
    battery: BatteryFrame,
    pressed: HashSet<Button>,
}

static SNAPSHOT: OnceLock<Mutex<ControllerSnapshot>> = OnceLock::new();
static VISIBLE: AtomicBool = AtomicBool::new(false);

fn snapshot_state() -> &'static Mutex<ControllerSnapshot> {
    SNAPSHOT.get_or_init(|| Mutex::new(ControllerSnapshot::default()))
}

/// Called by the single controller poll loop. The Center never opens SDL itself,
/// so it cannot steal a controller from gameplay or create a second device reader.
pub fn update(
    connected: bool,
    name: &str,
    input: &str,
    axes: (f32, f32, f32, f32),
    battery: BatteryFrame,
    changes: &[ButtonChange],
) {
    let Ok(mut state) = snapshot_state().lock() else {
        return;
    };
    state.connected = connected;
    state.name.clear();
    state.name.push_str(name);
    state.input.clear();
    state.input.push_str(input);
    state.axes = axes;
    state.battery = battery;
    if !connected {
        state.pressed.clear();
        return;
    }
    for change in changes {
        if change.pressed {
            state.pressed.insert(change.button);
        } else {
            state.pressed.remove(&change.button);
        }
    }
}

pub fn is_visible() -> bool {
    VISIBLE.load(Ordering::SeqCst)
}

struct CenterController {
    thread: Option<DesktopWindowThread>,
}

impl Default for CenterController {
    fn default() -> Self {
        Self { thread: None }
    }
}

static CONTROLLER: OnceLock<Mutex<CenterController>> = OnceLock::new();

pub fn show() -> Result<(), String> {
    controller_shortcuts::reload();
    let controller = CONTROLLER.get_or_init(|| Mutex::new(CenterController::default()));
    let mut controller = controller
        .lock()
        .map_err(|_| "Controller Center thread lock poisoned".to_string())?;
    if controller.thread.is_none() {
        controller.thread = Some(desktop_window::spawn(CenterApp)?);
    }
    controller
        .thread
        .as_ref()
        .expect("Controller Center thread was created")
        .show(LPARAM(0))
}

struct CenterApp;

impl DesktopApp for CenterApp {
    const THREAD_NAME: &'static str = "warmup-controller-center";
    const CLASS_NAME: PCWSTR = WINDOW_CLASS;
    const BG_COLOR: u32 = WINDOW_BG;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = center_wndproc;

    fn on_show(&mut self, _lparam: LPARAM) {
        ui_show();
    }

    fn on_hide(&mut self) {
        ui_hide();
    }
}

#[derive(Default)]
struct CenterUi {
    hwnd: Option<HWND>,
    renderer: Option<VkRenderer>,
    selected: Option<Button>,
    selected_hold: Option<Button>,
    editor_mode: Option<DesktopActionKind>,
    editor_text: String,
    deadzone: f32,
    notice: String,
}

thread_local! {
    static UI: RefCell<CenterUi> = RefCell::new(CenterUi::default());
}

fn ui_show() {
    let hwnd = UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        if ui.hwnd.is_none() {
            let hwnd = match unsafe { create_center_window() } {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    crate::install::log_line(&format!("controller center: create window: {error}"));
                    return None;
                }
            };
            ui.renderer = match unsafe { VkRenderer::create(hwnd) } {
                Ok(renderer) => Some(renderer),
                Err(error) => {
                    crate::install::log_line(&format!("controller center: renderer init: {error}"));
                    None
                }
            };
            ui.hwnd = Some(hwnd);
        }
        ui.deadzone = crate::config::gamepad_settings().cursor_deadzone;
        ui.notice.clear();
        ui.hwnd
    });
    let Some(hwnd) = hwnd else {
        return;
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetTimer(hwnd, REPAINT_TIMER_ID, REPAINT_TIMER_MS, None);
    }
    VISIBLE.store(true, Ordering::SeqCst);
    render_center(hwnd);
}

fn ui_hide() {
    UI.with(|slot| {
        let ui = slot.borrow();
        if let Some(hwnd) = ui.hwnd {
            unsafe {
                let _ = KillTimer(hwnd, REPAINT_TIMER_ID);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    });
    VISIBLE.store(false, Ordering::SeqCst);
}

unsafe fn create_center_window() -> Result<HWND, String> {
    let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP,
        WINDOW_CLASS,
        w!("Warmup Controller Center"),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        1120,
        760,
        None,
        HMENU::default(),
        windows::Win32::Foundation::HINSTANCE(instance.0),
        None,
    )
    .map_err(|e| format!("CreateWindowExW: {e}"))
}

unsafe extern "system" fn center_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            render_center(hwnd);
            let _ = ValidateRect(hwnd, None);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == REPAINT_TIMER_ID => {
            render_center(hwnd);
            LRESULT(0)
        }
        WM_SIZE => {
            render_center(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            handle_click(hwnd, lparam);
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            handle_key(wparam.0 as u16);
            render_center(hwnd);
            LRESULT(0)
        }
        WM_CHAR => {
            capture_editor_char(wparam.0 as u16);
            render_center(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            ui_hide();
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, REPAINT_TIMER_ID);
            UI.with(|slot| {
                let mut ui = slot.borrow_mut();
                if ui.hwnd == Some(hwnd) {
                    ui.hwnd = None;
                    ui.renderer = None;
                }
            });
            VISIBLE.store(false, Ordering::SeqCst);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn handle_click(hwnd: HWND, lparam: LPARAM) {
    let raw = lparam.0 as u32;
    let x = (raw & 0xffff) as u16 as i16 as f32;
    let y = (raw >> 16) as u16 as i16 as f32;
    let mut rect = Default::default();
    unsafe {
        if GetClientRect(hwnd, &mut rect).is_err() {
            return;
        }
    }
    let width = (rect.right - rect.left).max(1) as f32;
    let height = (rect.bottom - rect.top).max(1) as f32;
    let selected = UI.with(|slot| slot.borrow().selected);
    match controller_center_hit(x, y, width, height, selected) {
        Some(ControllerCenterHit::Button(button)) => {
            select_button(button);
            unsafe {
                let _ = SetFocus(hwnd);
            }
        }
        Some(ControllerCenterHit::Deadzone(percent)) => {
            let deadzone = percent as f32 / 100.0;
            match crate::config::set_gamepad_setting("cursor_deadzone", &deadzone.to_string()) {
                Ok(()) => UI.with(|slot| slot.borrow_mut().deadzone = deadzone),
                Err(error) => UI.with(|slot| {
                    slot.borrow_mut().notice = format!("Could not save deadzone: {error}")
                }),
            }
        }
        Some(ControllerCenterHit::Action(kind)) => select_action_kind(kind),
        Some(ControllerCenterHit::Modifier(hold)) => select_modifier(hold),
        Some(ControllerCenterHit::Clear) => clear_selected_mapping(),
        None => {}
    }
    render_center(hwnd);
}

fn load_editor(hold: Option<Button>, press: Button) -> (Option<DesktopActionKind>, String) {
    match selected_mapping(hold, press) {
        Some(ControllerAction::Launch(value)) => (Some(DesktopActionKind::Launch), value),
        Some(ControllerAction::Workspace(value)) => (Some(DesktopActionKind::Workspace), value),
        Some(ControllerAction::Command(value)) => (Some(DesktopActionKind::Command), value),
        _ => (Some(DesktopActionKind::Shortcut), String::new()),
    }
}

fn select_button(button: Button) {
    let (editor_mode, editor_text) = load_editor(None, button);
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        ui.selected = Some(button);
        ui.selected_hold = None;
        ui.editor_mode = editor_mode;
        ui.editor_text = editor_text;
        ui.notice.clear();
    });
}

fn select_modifier(hold: Option<Button>) {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(press) = ui.selected else {
            return;
        };
        let (editor_mode, editor_text) = load_editor(hold, press);
        ui.selected_hold = hold;
        ui.editor_mode = editor_mode;
        ui.editor_text = editor_text;
        ui.notice.clear();
    });
}

fn select_action_kind(kind: DesktopActionKind) {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(button) = ui.selected else {
            return;
        };
        ui.editor_mode = Some(kind);
        ui.editor_text = selected_mapping(ui.selected_hold, button)
            .filter(|action| action.kind() == kind)
            .and_then(|action| action.value().map(str::to_string))
            .unwrap_or_default();
        ui.notice.clear();
    });
}

fn clear_selected_mapping() {
    let selected = UI.with(|slot| {
        let ui = slot.borrow();
        ui.selected.map(|button| (ui.selected_hold, button))
    });
    let Some((hold, button)) = selected else {
        return;
    };
    let result = set_selected_mapping(hold, button, None);
    let shortcut = selected_shortcut(hold, button);
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        ui.selected = None;
        ui.selected_hold = None;
        ui.editor_mode = None;
        ui.editor_text.clear();
        ui.notice = match result {
            Ok(()) => format!("{shortcut} mapping cleared"),
            Err(error) => format!("Could not clear mapping: {error}"),
        };
    });
}

fn handle_key(vk: u16) {
    let editor_mode = UI.with(|slot| slot.borrow().editor_mode);
    if matches!(
        editor_mode,
        Some(DesktopActionKind::Launch | DesktopActionKind::Workspace | DesktopActionKind::Command)
    ) {
        edit_action(vk);
    } else {
        capture_shortcut(vk);
    }
}

fn capture_shortcut(vk: u16) {
    let selected = UI.with(|slot| {
        let ui = slot.borrow();
        ui.selected.map(|button| (ui.selected_hold, button))
    });
    let Some((hold, button)) = selected else {
        return;
    };
    if vk == VK_ESCAPE.0 {
        UI.with(|slot| {
            let mut ui = slot.borrow_mut();
            ui.selected = None;
            ui.notice.clear();
        });
        return;
    }
    if vk == VK_BACK.0 || vk == 0x2e {
        clear_selected_mapping();
        return;
    }
    if is_modifier_key(vk) {
        return;
    }
    let shortcut = Shortcut::new(
        vk,
        key_down(VK_CONTROL),
        key_down(VK_MENU),
        key_down(VK_SHIFT),
        key_down(VK_LWIN) || key_down(VK_RWIN),
    );
    let result = shortcut
        .ok_or_else(|| "That key cannot be used as the shortcut key".to_string())
        .and_then(|shortcut| {
            set_selected_mapping(hold, button, Some(ControllerAction::Shortcut(shortcut)))
                .map(|_| shortcut)
        });
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        ui.selected = None;
        ui.selected_hold = None;
        ui.editor_mode = None;
        ui.editor_text.clear();
        ui.notice = match result {
            Ok(shortcut) => format!(
                "{} mapped to {}",
                selected_shortcut(hold, button),
                shortcut.display()
            ),
            Err(error) => format!("Could not save mapping: {error}"),
        };
    });
}

fn edit_action(vk: u16) {
    match vk {
        value if value == VK_ESCAPE.0 => {
            UI.with(|slot| {
                let mut ui = slot.borrow_mut();
                ui.selected = None;
                ui.selected_hold = None;
                ui.editor_mode = None;
                ui.editor_text.clear();
                ui.notice.clear();
            });
        }
        value if value == VK_BACK.0 => {
            UI.with(|slot| {
                slot.borrow_mut().editor_text.pop();
            });
        }
        0x0d => save_text_action(),
        _ => {}
    }
}

fn capture_editor_char(unit: u16) {
    let Some(character) = char::from_u32(unit as u32) else {
        return;
    };
    if character.is_control() {
        return;
    }
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        if matches!(
            ui.editor_mode,
            Some(
                DesktopActionKind::Launch
                    | DesktopActionKind::Workspace
                    | DesktopActionKind::Command
            )
        ) && ui.editor_text.len() + character.len_utf8() <= 1024
        {
            ui.editor_text.push(character);
        }
    });
}

fn save_text_action() {
    let (hold, button, kind, text) = UI.with(|slot| {
        let ui = slot.borrow();
        (
            ui.selected_hold,
            ui.selected,
            ui.editor_mode,
            ui.editor_text.clone(),
        )
    });
    let (Some(button), Some(kind)) = (button, kind) else {
        return;
    };
    let result = ControllerAction::new(kind, &text)
        .ok_or_else(|| "Enter a valid shortcut, target, workspace name, or command".to_string())
        .and_then(|action| {
            if kind == DesktopActionKind::Workspace {
                controller_shortcuts::capture_workspace(&text)?;
            }
            set_selected_mapping(hold, button, Some(action))
        });
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        match result {
            Ok(()) => {
                ui.selected = None;
                ui.selected_hold = None;
                ui.editor_mode = None;
                ui.editor_text.clear();
                ui.notice = format!("{} mapping saved", selected_shortcut(hold, button));
            }
            Err(error) => ui.notice = format!("Could not save mapping: {error}"),
        }
    });
}

fn selected_mapping(hold: Option<Button>, press: Button) -> Option<ControllerAction> {
    match hold {
        Some(hold) => {
            let chord = ControllerChord::new(hold, press)?;
            controller_shortcuts::chord_mapping(chord.hold, chord.press)
        }
        None => controller_shortcuts::mapping(press),
    }
}

fn set_selected_mapping(
    hold: Option<Button>,
    press: Button,
    action: Option<ControllerAction>,
) -> Result<(), String> {
    match hold {
        Some(hold) => {
            let chord = ControllerChord::new(hold, press)
                .ok_or_else(|| "Choose two distinct controller buttons".to_string())?;
            controller_shortcuts::chord_setting_key(chord.hold, chord.press)
                .ok_or_else(|| "Choose two distinct controller buttons".to_string())?;
            controller_shortcuts::set_chord_mapping(chord.hold, chord.press, action)
        }
        None => controller_shortcuts::set_mapping(press, action),
    }
}

fn selected_shortcut(hold: Option<Button>, press: Button) -> String {
    hold.map_or_else(
        || press.as_str().to_string(),
        |hold| format!("{} + {}", hold.as_str(), press.as_str()),
    )
}

fn key_down(key: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(key.0 as i32) < 0 }
}

fn is_modifier_key(vk: u16) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5b | 0x5c | 0xa0..=0xa5)
}

fn render_center(hwnd: HWND) {
    let snapshot = snapshot_state()
        .lock()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default();
    let (selected, selected_hold, editor_mode, editor_text, deadzone, notice) = UI.with(|slot| {
        let ui = slot.borrow();
        (
            ui.selected,
            ui.selected_hold,
            ui.editor_mode,
            ui.editor_text.clone(),
            ui.deadzone,
            ui.notice.clone(),
        )
    });
    let actions: Vec<(Button, String, bool)> = MAPPABLE_BUTTONS
        .iter()
        .map(|&button| {
            let action = controller_shortcuts::mapping(button)
                .map(|action| compact_action_label(&action))
                .unwrap_or_else(|| match default_action(button) {
                    "Default" => "Default".to_string(),
                    action => format!("Default · {action}"),
                });
            (button, action, snapshot.pressed.contains(&button))
        })
        .collect();
    let bindings: Vec<ControllerCenterBinding<'_>> = actions
        .iter()
        .map(|(button, action, pressed)| ControllerCenterBinding {
            button: *button,
            action,
            pressed: *pressed,
        })
        .collect();
    let input = if notice.is_empty() {
        snapshot.input.as_str()
    } else {
        notice.as_str()
    };
    let palette = super::vk_ui::current_vk_palette();
    let frame = ControllerCenterFrame {
        palette: &palette,
        connected: snapshot.connected,
        controller_label: &snapshot.name,
        input,
        battery_percent: snapshot.battery.percent,
        charging: snapshot.battery.charging,
        wired: snapshot.battery.wired,
        axes: snapshot.axes,
        bindings: &bindings,
        selected,
        selected_hold,
        editor_mode,
        editor_text: &editor_text,
        deadzone,
    };
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(renderer) = ui.renderer.as_mut() else {
            return;
        };
        unsafe {
            if let Err(error) = renderer.resize(hwnd) {
                crate::install::log_line(&format!("controller center: resize: {error}"));
                return;
            }
            if let Err(error) = renderer.draw_controller_center(&frame) {
                crate::install::log_line(&format!("controller center: draw: {error}"));
            }
        }
    });
}

fn compact_action_label(action: &ControllerAction) -> String {
    const MAX_CHARS: usize = 30;
    let label = action.display();
    let mut chars = label.chars();
    let mut compact = String::new();
    for _ in 0..MAX_CHARS {
        let Some(character) = chars.next() else {
            return label;
        };
        compact.push(character);
    }
    compact.push('…');
    compact
}

fn default_action(button: Button) -> &'static str {
    match button {
        Button::A | Button::Touchpad => "left click",
        Button::B => "right click",
        Button::Select | Button::Start => "Enter",
        Button::L3 => "open keyboard",
        Button::R3 => "voice typing",
        Button::Guide => "open warmUP",
        _ => "Default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_shortcut_keeps_single_and_directed_chord_order() {
        assert_eq!(selected_shortcut(None, Button::A), "A");
        assert_eq!(selected_shortcut(Some(Button::Lb), Button::A), "LB + A");
        assert_ne!(
            selected_shortcut(Some(Button::A), Button::Lb),
            selected_shortcut(Some(Button::Lb), Button::A)
        );
        assert!(ControllerChord::new(Button::A, Button::A).is_none());
    }
}
