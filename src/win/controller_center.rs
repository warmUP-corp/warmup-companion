//! Native Controller Center window, rendered through the existing D3D/D2D path.

use std::cell::RefCell;
use std::collections::HashSet;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR,
};
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LWIN, VK_MENU,
    VK_RWIN, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, KillTimer, SetForegroundWindow, SetTimer,
    ShowWindow, CW_USEDEFAULT, HICON, HMENU, SW_HIDE, SW_SHOWNORMAL, WM_CHAR, WM_CLOSE, WM_DESTROY,
    WM_KEYDOWN, WM_LBUTTONUP, WM_PAINT, WM_SIZE, WM_SYSKEYDOWN, WM_TIMER,
    WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW,
};

use crate::controller_shortcuts::{
    self, ControllerAction, ControllerChord, DesktopActionKind, LaunchableApp, Shortcut,
    WorkspaceWindowCandidate, MAPPABLE_BUTTONS,
};
use crate::gamepad_backend::{BatteryFrame, Button, ButtonChange};

use super::desktop_window::{self, DesktopApp, DesktopWindowThread};
use super::vk_renderer::{
    controller_center_hit, controller_center_hit_with_wizard, ControllerCenterBinding,
    ControllerCenterFrame, ControllerCenterHit, ControllerCenterHitState, ControllerCenterStep,
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
    capture_events: Vec<TriggerInput>,
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
        state.capture_events.clear();
        return;
    }
    for change in changes {
        let held_before = if change.pressed {
            MAPPABLE_BUTTONS
                .iter()
                .copied()
                .find(|button| *button != change.button && state.pressed.contains(button))
        } else {
            None
        };
        if is_visible() {
            if state.capture_events.len() >= 64 {
                state.capture_events.remove(0);
            }
            state.capture_events.push(TriggerInput {
                button: change.button,
                pressed: change.pressed,
                held_before,
            });
        }
        if change.pressed {
            state.pressed.insert(change.button);
        } else {
            state.pressed.remove(&change.button);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TriggerInput {
    button: Button,
    pressed: bool,
    held_before: Option<Button>,
}

fn take_capture_events() -> Vec<TriggerInput> {
    snapshot_state()
        .lock()
        .map(|mut state| std::mem::take(&mut state.capture_events))
        .unwrap_or_default()
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

    fn window_icon(&self) -> HICON {
        unsafe { crate::tray::load_tray_icon() }
    }

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
    wizard: Option<WizardState>,
    deadzone: f32,
    notice: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TriggerSelection {
    hold: Option<Button>,
    press: Button,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TriggerCaptureState {
    selection: TriggerSelection,
    pending_single: Option<Button>,
}

impl TriggerCaptureState {
    fn new(button: Button) -> Self {
        Self {
            selection: TriggerSelection {
                hold: None,
                press: button,
            },
            pending_single: None,
        }
    }
}

fn apply_trigger_input(state: &mut TriggerCaptureState, input: TriggerInput) {
    if input.pressed {
        if let Some(pending) = state.pending_single {
            if pending != input.button {
                state.selection = TriggerSelection {
                    hold: Some(pending),
                    press: input.button,
                };
                state.pending_single = None;
            }
        } else if let Some(hold) = input.held_before {
            if hold != input.button {
                state.selection = TriggerSelection {
                    hold: Some(hold),
                    press: input.button,
                };
            }
        } else {
            state.pending_single = Some(input.button);
        }
    } else if state.pending_single == Some(input.button) {
        state.selection = TriggerSelection {
            hold: None,
            press: input.button,
        };
        state.pending_single = None;
    }
}

#[derive(Clone)]
struct WizardState {
    trigger: TriggerCaptureState,
    step: ControllerCenterStep,
    action: Option<DesktopActionKind>,
    shortcut: Option<Shortcut>,
    launch_target: String,
    app_query: String,
    apps: Vec<LaunchableApp>,
    app_matches: Vec<usize>,
    app_selected: Option<usize>,
    app_scroll: usize,
    workspace_name: String,
    workspace_candidates: Vec<WorkspaceWindowCandidate>,
    workspace_selected: HashSet<isize>,
    workspace_scroll: usize,
    command_text: String,
    notice: String,
    overwrite_confirmed: bool,
}

impl WizardState {
    fn new(button: Button) -> Self {
        let mut wizard = Self {
            trigger: TriggerCaptureState::new(button),
            step: ControllerCenterStep::Trigger,
            action: None,
            shortcut: None,
            launch_target: String::new(),
            app_query: String::new(),
            apps: Vec::new(),
            app_matches: Vec::new(),
            app_selected: None,
            app_scroll: 0,
            workspace_name: String::new(),
            workspace_candidates: Vec::new(),
            workspace_selected: HashSet::new(),
            workspace_scroll: 0,
            command_text: String::new(),
            notice: String::new(),
            overwrite_confirmed: false,
        };
        wizard.load_existing_action();
        wizard
    }

    fn load_existing_action(&mut self) {
        self.action = None;
        self.shortcut = None;
        self.launch_target.clear();
        self.app_query.clear();
        self.apps.clear();
        self.app_matches.clear();
        self.app_selected = None;
        self.app_scroll = 0;
        self.workspace_name.clear();
        self.workspace_candidates.clear();
        self.workspace_selected.clear();
        self.workspace_scroll = 0;
        self.command_text.clear();
        self.overwrite_confirmed = false;
        let Some(action) =
            selected_mapping(self.trigger.selection.hold, self.trigger.selection.press)
        else {
            return;
        };
        self.action = Some(action.kind());
        match action {
            ControllerAction::Shortcut(shortcut) => self.shortcut = Some(shortcut),
            ControllerAction::Launch(target) => self.launch_target = target,
            ControllerAction::Workspace(name) => self.workspace_name = name,
            ControllerAction::Command(command) => self.command_text = command,
        }
    }
}

thread_local! {
    static UI: RefCell<CenterUi> = RefCell::new(CenterUi::default());
}

fn ui_show() {
    let _ = take_capture_events();
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
        ui.wizard = None;
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
        let mut ui = slot.borrow_mut();
        ui.wizard = None;
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
    let hwnd = CreateWindowExW(
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
    .map_err(|e| format!("CreateWindowExW: {e}"))?;
    let caption_color = COLORREF(WINDOW_BG);
    let caption_color_ptr = &caption_color as *const COLORREF as *const _;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_CAPTION_COLOR,
        caption_color_ptr,
        size_of::<COLORREF>() as u32,
    );
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        caption_color_ptr,
        size_of::<COLORREF>() as u32,
    );
    Ok(hwnd)
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
            capture_wizard_char(wparam.0 as u16);
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
    let (selected, wizard_state) = UI.with(|slot| {
        let ui = slot.borrow();
        let selected = ui
            .wizard
            .as_ref()
            .map(|wizard| wizard.trigger.selection.press);
        let state = ui.wizard.as_ref().map(wizard_hit_state);
        (selected, state)
    });
    let hit = wizard_state.map_or_else(
        || controller_center_hit(x, y, width, height, selected),
        |state| controller_center_hit_with_wizard(x, y, width, height, selected, state),
    );
    match hit {
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
        Some(ControllerCenterHit::Continue) => continue_wizard(),
        Some(ControllerCenterHit::Back) => back_wizard(),
        Some(ControllerCenterHit::Cancel) => cancel_wizard(),
        Some(ControllerCenterHit::Save) => save_wizard(),
        Some(ControllerCenterHit::Clear) => clear_selected_mapping(),
        Some(ControllerCenterHit::AppRow(row)) => select_app_row(row),
        Some(ControllerCenterHit::AppScrollUp) => scroll_app(-1),
        Some(ControllerCenterHit::AppScrollDown) => scroll_app(1),
        Some(ControllerCenterHit::WorkspaceRow(row)) => toggle_workspace_row(row),
        Some(ControllerCenterHit::WorkspaceScrollUp) => scroll_workspace(-1),
        Some(ControllerCenterHit::WorkspaceScrollDown) => scroll_workspace(1),
        Some(
            ControllerCenterHit::TriggerCapture
            | ControllerCenterHit::ShortcutCapture
            | ControllerCenterHit::AppSearch
            | ControllerCenterHit::CommandInput
            | ControllerCenterHit::WorkspaceName,
        ) => {}
        None => {}
    }
    render_center(hwnd);
}

fn wizard_hit_state(wizard: &WizardState) -> ControllerCenterHitState {
    let app_rows = wizard.app_matches.len();
    ControllerCenterHitState {
        step: wizard.step,
        action: wizard.action,
        app_rows,
        app_can_scroll_up: wizard.app_scroll > 0,
        app_can_scroll_down: wizard.app_scroll + 3 < app_rows,
        workspace_rows: wizard.workspace_candidates.len(),
        workspace_can_scroll_up: wizard.workspace_scroll > 0,
        workspace_can_scroll_down: wizard.workspace_scroll + 3 < wizard.workspace_candidates.len(),
    }
}

fn select_button(button: Button) {
    let _ = take_capture_events();
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        ui.wizard = Some(WizardState::new(button));
        ui.notice.clear();
    });
}

fn select_action_kind(kind: DesktopActionKind) {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        if wizard.step != ControllerCenterStep::Action
            || !matches!(
                kind,
                DesktopActionKind::Shortcut
                    | DesktopActionKind::Launch
                    | DesktopActionKind::Workspace
            )
        {
            return;
        }
        wizard.action = Some(kind);
        wizard.overwrite_confirmed = false;
        wizard.notice.clear();
        ui.notice.clear();
    });
}

fn continue_wizard() {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        match wizard.step {
            ControllerCenterStep::Trigger => {
                wizard.step = ControllerCenterStep::Action;
                wizard.overwrite_confirmed = false;
                wizard.notice.clear();
            }
            ControllerCenterStep::Action if wizard.action.is_some() => {
                wizard.step = ControllerCenterStep::Configure;
                wizard.overwrite_confirmed = false;
                wizard.notice.clear();
            }
            ControllerCenterStep::Action => {
                wizard.notice = "Choose Keyboard shortcut, Open app, or Restore workspace.".into();
            }
            ControllerCenterStep::Configure => {}
        }
    });
    let should_refresh = UI.with(|slot| {
        slot.borrow()
            .wizard
            .as_ref()
            .is_some_and(|wizard| wizard.step == ControllerCenterStep::Configure)
    });
    if should_refresh {
        refresh_configure_inventory();
    }
}

fn back_wizard() {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        let moved = match wizard.step {
            ControllerCenterStep::Configure => {
                wizard.step = ControllerCenterStep::Action;
                true
            }
            ControllerCenterStep::Action => {
                wizard.step = ControllerCenterStep::Trigger;
                true
            }
            ControllerCenterStep::Trigger => {
                wizard.notice =
                    "The first step has no previous page. Choose Cancel to close.".into();
                false
            }
        };
        if moved {
            wizard.overwrite_confirmed = false;
            wizard.notice.clear();
        }
    });
}

fn cancel_wizard() {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        if ui.wizard.take().is_some() {
            ui.notice = "Mapping setup cancelled".into();
        }
    });
    let _ = take_capture_events();
}

fn clear_selected_mapping() {
    let selected = UI.with(|slot| {
        let ui = slot.borrow();
        ui.wizard.as_ref().map(|wizard| {
            (
                wizard.trigger.selection.hold,
                wizard.trigger.selection.press,
            )
        })
    });
    let Some((hold, button)) = selected else {
        return;
    };
    let result = set_selected_mapping(hold, button, None);
    let shortcut = selected_shortcut(hold, button);
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let message = match result {
            Ok(()) => {
                if let Some(wizard) = ui.wizard.as_mut() {
                    wizard.action = None;
                    wizard.shortcut = None;
                    wizard.launch_target.clear();
                    wizard.app_matches.clear();
                    wizard.app_selected = None;
                    wizard.workspace_name.clear();
                    wizard.workspace_selected.clear();
                    wizard.command_text.clear();
                    wizard.overwrite_confirmed = false;
                    if wizard.step == ControllerCenterStep::Configure {
                        wizard.step = ControllerCenterStep::Action;
                    }
                    wizard.notice = format!("{shortcut} mapping cleared");
                }
                format!("{shortcut} mapping cleared")
            }
            Err(error) => format!("Could not clear mapping: {error}"),
        };
        ui.notice = message;
    });
}

fn handle_key(vk: u16) {
    let (step, action) = UI.with(|slot| {
        slot.borrow()
            .wizard
            .as_ref()
            .map(|wizard| (Some(wizard.step), wizard.action))
            .unwrap_or((None, None))
    });
    if vk == VK_ESCAPE.0 {
        cancel_wizard();
        return;
    }
    if vk == 0x0d {
        activate_primary(step, action);
        return;
    }
    if vk == VK_BACK.0 {
        edit_wizard_backspace(step, action);
        return;
    }
    if matches!(
        (step, action),
        (
            Some(ControllerCenterStep::Configure),
            Some(DesktopActionKind::Launch | DesktopActionKind::Workspace)
        )
    ) && (vk == VK_UP.0 || vk == VK_DOWN.0)
    {
        if action == Some(DesktopActionKind::Launch) {
            scroll_app(if vk == VK_UP.0 { -1 } else { 1 });
        } else {
            scroll_workspace(if vk == VK_UP.0 { -1 } else { 1 });
        }
        return;
    }
    if step == Some(ControllerCenterStep::Configure) && action == Some(DesktopActionKind::Shortcut)
    {
        capture_keyboard_shortcut(vk);
    }
}

fn activate_primary(step: Option<ControllerCenterStep>, action: Option<DesktopActionKind>) {
    match step {
        Some(ControllerCenterStep::Trigger) => continue_wizard(),
        Some(ControllerCenterStep::Action) => {
            if action.is_none() {
                select_action_kind(DesktopActionKind::Shortcut);
            }
            continue_wizard();
        }
        Some(ControllerCenterStep::Configure) => save_wizard(),
        None => {}
    }
}

fn edit_wizard_backspace(step: Option<ControllerCenterStep>, action: Option<DesktopActionKind>) {
    if step != Some(ControllerCenterStep::Configure) {
        return;
    }
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        match action {
            Some(DesktopActionKind::Launch) => {
                wizard.app_query.pop();
                wizard.app_matches = filtered_app_indices(&wizard.apps, &wizard.app_query);
                wizard.app_selected = None;
                wizard.app_scroll = 0;
            }
            Some(DesktopActionKind::Workspace) => {
                wizard.workspace_name.pop();
                wizard.overwrite_confirmed = false;
            }
            Some(DesktopActionKind::Command) => {
                wizard.command_text.pop();
            }
            Some(DesktopActionKind::Shortcut) => wizard.shortcut = None,
            None => {}
        }
    });
}

fn capture_keyboard_shortcut(vk: u16) {
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
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        wizard.shortcut = shortcut;
        wizard.notice = shortcut.map_or_else(
            || "That key cannot be used as a shortcut key".into(),
            |shortcut| format!("Captured {} · press Save to apply", shortcut.display()),
        );
    });
}

fn capture_wizard_char(unit: u16) {
    let Some(character) = char::from_u32(unit as u32) else {
        return;
    };
    if character.is_control() {
        return;
    }
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        if wizard.step != ControllerCenterStep::Configure {
            return;
        }
        match wizard.action {
            Some(DesktopActionKind::Launch)
                if wizard.app_query.len() + character.len_utf8() <= 256 =>
            {
                wizard.app_query.push(character);
                wizard.app_matches = filtered_app_indices(&wizard.apps, &wizard.app_query);
                wizard.app_selected = None;
                wizard.app_scroll = 0;
            }
            Some(DesktopActionKind::Workspace)
                if wizard.workspace_name.len() + character.len_utf8() <= 48 =>
            {
                wizard.workspace_name.push(character);
                wizard.overwrite_confirmed = false;
            }
            Some(DesktopActionKind::Command)
                if wizard.command_text.len() + character.len_utf8() <= 1024 =>
            {
                wizard.command_text.push(character);
            }
            _ => {}
        }
    });
}

fn refresh_configure_inventory() {
    let (action, hwnd) = UI.with(|slot| {
        let ui = slot.borrow();
        (ui.wizard.as_ref().and_then(|wizard| wizard.action), ui.hwnd)
    });
    match action {
        Some(DesktopActionKind::Launch) => {
            let apps = controller_shortcuts::launchable_apps();
            UI.with(|slot| {
                let mut ui = slot.borrow_mut();
                let Some(wizard) = ui.wizard.as_mut() else {
                    return;
                };
                wizard.app_selected = apps
                    .iter()
                    .position(|app| app.target == wizard.launch_target);
                wizard.app_matches = filtered_app_indices(&apps, &wizard.app_query);
                wizard.apps = apps;
                wizard.app_scroll = 0;
                wizard.overwrite_confirmed = false;
                if wizard.apps.is_empty() {
                    wizard.notice =
                        "No launchable apps found. Back, then Continue to refresh after installing an app."
                            .into();
                }
            });
        }
        Some(DesktopActionKind::Workspace) => {
            let center_id = hwnd.map(|hwnd| hwnd.0 as isize);
            let candidates = controller_shortcuts::workspace_window_candidates()
                .into_iter()
                .filter(|candidate| Some(candidate.id) != center_id)
                .collect::<Vec<_>>();
            UI.with(|slot| {
                let mut ui = slot.borrow_mut();
                let Some(wizard) = ui.wizard.as_mut() else {
                    return;
                };
                wizard
                    .workspace_selected
                    .retain(|id| candidates.iter().any(|candidate| candidate.id == *id));
                wizard.workspace_candidates = candidates;
                wizard.workspace_scroll = 0;
                wizard.overwrite_confirmed = false;
                if wizard.workspace_candidates.is_empty() {
                    wizard.notice =
                        "No eligible visible windows found. Back, then Continue to refresh after opening an app."
                            .into();
                }
            });
        }
        _ => {}
    }
}

fn filtered_app_indices(apps: &[LaunchableApp], query: &str) -> Vec<usize> {
    let query = query.trim().to_ascii_lowercase();
    apps.iter()
        .enumerate()
        .filter(|(_, app)| {
            query.is_empty()
                || app.name.to_ascii_lowercase().contains(&query)
                || app.target.to_ascii_lowercase().contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

fn select_app_row(row: usize) {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        if wizard.step != ControllerCenterStep::Configure
            || wizard.action != Some(DesktopActionKind::Launch)
        {
            return;
        }
        if let Some(index) = wizard.app_matches.get(wizard.app_scroll + row).copied() {
            wizard.app_selected = Some(index);
            wizard.launch_target = wizard.apps[index].target.clone();
            wizard.overwrite_confirmed = false;
            wizard.notice.clear();
        }
    });
}

fn scroll_app(delta: isize) {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        let count = wizard.app_matches.len();
        let max_scroll = count.saturating_sub(3);
        wizard.app_scroll = if delta.is_negative() {
            wizard.app_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            (wizard.app_scroll + delta as usize).min(max_scroll)
        };
        wizard.overwrite_confirmed = false;
    });
}

fn toggle_workspace_row(row: usize) {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        if wizard.step != ControllerCenterStep::Configure
            || wizard.action != Some(DesktopActionKind::Workspace)
        {
            return;
        }
        let Some(candidate) = wizard
            .workspace_candidates
            .get(wizard.workspace_scroll + row)
        else {
            return;
        };
        if !wizard.workspace_selected.insert(candidate.id) {
            wizard.workspace_selected.remove(&candidate.id);
        }
        wizard.overwrite_confirmed = false;
        wizard.notice.clear();
    });
}

fn scroll_workspace(delta: isize) {
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(wizard) = ui.wizard.as_mut() else {
            return;
        };
        let max_scroll = wizard.workspace_candidates.len().saturating_sub(3);
        wizard.workspace_scroll = if delta.is_negative() {
            wizard.workspace_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            (wizard.workspace_scroll + delta as usize).min(max_scroll)
        };
        wizard.overwrite_confirmed = false;
    });
}

fn workspace_setting_key(name: &str) -> String {
    let slug = name
        .trim()
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' => (byte + 32) as char,
            b'a'..=b'z' | b'0'..=b'9' | b'_' => byte as char,
            b' ' | b'-' => '_',
            _ => '_',
        })
        .collect::<String>();
    format!("workspace_{slug}")
}

fn workspace_name_exists(name: &str) -> bool {
    let Some(path) = crate::config::settings_path() else {
        return false;
    };
    let key = workspace_setting_key(name);
    std::fs::read_to_string(path).ok().is_some_and(|text| {
        text.lines().any(|line| {
            line.split_once('=')
                .is_some_and(|(candidate, _)| candidate.trim() == key)
        })
    })
}

fn save_wizard() {
    let Some(wizard) = UI.with(|slot| slot.borrow().wizard.clone()) else {
        return;
    };
    let trigger = wizard.trigger.selection;
    let duplicate_workspace = wizard.action == Some(DesktopActionKind::Workspace)
        && ControllerAction::new(DesktopActionKind::Workspace, wizard.workspace_name.trim())
            .is_some()
        && workspace_name_exists(&wizard.workspace_name);
    if duplicate_workspace && !wizard.overwrite_confirmed {
        UI.with(|slot| {
            let mut ui = slot.borrow_mut();
            if let Some(wizard) = ui.wizard.as_mut() {
                wizard.overwrite_confirmed = true;
                wizard.notice = format!(
                    "Workspace \"{}\" already exists. Press Save again to replace it.",
                    wizard.workspace_name.trim()
                );
            }
        });
        return;
    }
    let result = match wizard.action {
        Some(DesktopActionKind::Shortcut) => wizard
            .shortcut
            .map(ControllerAction::Shortcut)
            .ok_or_else(|| "Capture a keyboard shortcut before saving".to_string()),
        Some(DesktopActionKind::Launch) => {
            let index = wizard
                .app_selected
                .ok_or_else(|| "Select an app from the list before saving".to_string());
            index.and_then(|index| {
                let app = wizard.apps.get(index).ok_or_else(|| {
                    "The selected app is no longer available; refresh the list".to_string()
                })?;
                Ok(ControllerAction::Launch(app.target.clone()))
            })
        }
        Some(DesktopActionKind::Workspace) => {
            let name = wizard.workspace_name.trim();
            let action =
                ControllerAction::new(DesktopActionKind::Workspace, name).ok_or_else(|| {
                    "Use a workspace name with letters, numbers, spaces, - or _".to_string()
                });
            action.and_then(|action| {
                if wizard.workspace_selected.is_empty() {
                    return Err("Select at least one visible window to capture".to_string());
                }
                let ids = wizard
                    .workspace_selected
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                controller_shortcuts::capture_workspace_windows(name, &ids)?;
                Ok(action)
            })
        }
        Some(DesktopActionKind::Command) => {
            ControllerAction::new(DesktopActionKind::Command, &wizard.command_text)
                .ok_or_else(|| "The existing command mapping cannot be empty".to_string())
        }
        None => Err("Choose an action before saving".to_string()),
    };
    let result = result.and_then(|action| {
        set_selected_mapping(trigger.hold, trigger.press, Some(action.clone())).map(|_| action)
    });
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        match result {
            Ok(action) => {
                let message = if duplicate_workspace {
                    format!(
                        "{} mapped to {} · existing workspace updated",
                        selected_shortcut(trigger.hold, trigger.press),
                        action.display()
                    )
                } else {
                    format!(
                        "{} mapped to {}",
                        selected_shortcut(trigger.hold, trigger.press),
                        action.display()
                    )
                };
                ui.wizard = None;
                ui.notice = message;
            }
            Err(error) => {
                if let Some(wizard) = ui.wizard.as_mut() {
                    wizard.notice = format!("Could not save mapping: {error}");
                }
            }
        }
    });
}

fn apply_pending_trigger_events(wizard: &mut WizardState, events: &[TriggerInput]) {
    if wizard.step != ControllerCenterStep::Trigger {
        return;
    }
    for &event in events {
        let before = wizard.trigger.selection;
        apply_trigger_input(&mut wizard.trigger, event);
        if wizard.trigger.selection != before {
            wizard.load_existing_action();
        }
    }
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
    let capture_events = take_capture_events();
    UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        if let Some(wizard) = ui.wizard.as_mut() {
            apply_pending_trigger_events(wizard, &capture_events);
        }
    });
    let snapshot = snapshot_state()
        .lock()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default();
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
    let palette = super::vk_ui::current_vk_palette();
    let render_error = UI.with(|slot| {
        let mut ui = slot.borrow_mut();
        let Some(mut renderer) = ui.renderer.take() else {
            return None;
        };
        let result = {
            let empty_apps: &[LaunchableApp] = &[];
            let empty_matches: &[usize] = &[];
            let empty_candidates: &[WorkspaceWindowCandidate] = &[];
            let empty_selection = HashSet::new();
            let wizard = ui.wizard.as_ref();
            let input = if ui.notice.is_empty() {
                snapshot.input.as_str()
            } else {
                ui.notice.as_str()
            };
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
                selected: wizard.map(|wizard| wizard.trigger.selection.press),
                selected_hold: wizard.and_then(|wizard| wizard.trigger.selection.hold),
                wizard_pending: wizard.and_then(|wizard| wizard.trigger.pending_single),
                wizard_step: wizard.map(|wizard| wizard.step),
                wizard_action: wizard.and_then(|wizard| wizard.action),
                wizard_shortcut: wizard.and_then(|wizard| wizard.shortcut),
                launch_target: wizard.map_or("", |wizard| wizard.launch_target.as_str()),
                app_query: wizard.map_or("", |wizard| wizard.app_query.as_str()),
                apps: wizard.map_or(empty_apps, |wizard| wizard.apps.as_slice()),
                app_matches: wizard.map_or(empty_matches, |wizard| wizard.app_matches.as_slice()),
                app_selected: wizard.and_then(|wizard| wizard.app_selected),
                app_scroll: wizard.map_or(0, |wizard| wizard.app_scroll),
                workspace_name: wizard.map_or("", |wizard| wizard.workspace_name.as_str()),
                workspace_candidates: wizard.map_or(empty_candidates, |wizard| {
                    wizard.workspace_candidates.as_slice()
                }),
                workspace_selected_ids: wizard
                    .map_or(&empty_selection, |wizard| &wizard.workspace_selected),
                workspace_scroll: wizard.map_or(0, |wizard| wizard.workspace_scroll),
                command_text: wizard.map_or("", |wizard| wizard.command_text.as_str()),
                wizard_notice: wizard.map_or("", |wizard| wizard.notice.as_str()),
                deadzone: ui.deadzone,
            };
            unsafe {
                renderer
                    .resize(hwnd)
                    .and_then(|_| renderer.draw_controller_center(&frame))
            }
        };
        ui.renderer = Some(renderer);
        result.err()
    });
    if let Some(error) = render_error {
        crate::install::log_line(&format!("controller center: render: {error}"));
    }
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

    #[test]
    fn trigger_capture_waits_for_release_before_committing_a_single() {
        let mut state = TriggerCaptureState::new(Button::A);
        apply_trigger_input(
            &mut state,
            TriggerInput {
                button: Button::A,
                pressed: true,
                held_before: None,
            },
        );
        assert_eq!(state.pending_single, Some(Button::A));
        assert_eq!(state.selection.hold, None);

        apply_trigger_input(
            &mut state,
            TriggerInput {
                button: Button::A,
                pressed: false,
                held_before: None,
            },
        );
        assert_eq!(state.pending_single, None);
        assert_eq!(
            state.selection,
            TriggerSelection {
                hold: None,
                press: Button::A
            }
        );
    }

    #[test]
    fn trigger_capture_turns_a_second_press_into_a_directed_chord() {
        let mut state = TriggerCaptureState::new(Button::A);
        apply_trigger_input(
            &mut state,
            TriggerInput {
                button: Button::Lb,
                pressed: true,
                held_before: None,
            },
        );
        apply_trigger_input(
            &mut state,
            TriggerInput {
                button: Button::A,
                pressed: true,
                held_before: Some(Button::Lb),
            },
        );
        assert_eq!(
            state.selection,
            TriggerSelection {
                hold: Some(Button::Lb),
                press: Button::A,
            }
        );
        assert_eq!(state.pending_single, None);
    }

    #[test]
    fn cached_app_matches_follow_query_edits_and_refresh() {
        let apps = vec![
            LaunchableApp {
                name: "Editor".into(),
                target: "C:/Apps/editor.exe".into(),
            },
            LaunchableApp {
                name: "Browser".into(),
                target: "C:/Apps/browser.exe".into(),
            },
        ];
        assert_eq!(filtered_app_indices(&apps, ""), vec![0, 1]);
        assert_eq!(filtered_app_indices(&apps, "EDITOR"), vec![0]);

        let refreshed = vec![LaunchableApp {
            name: "Mail".into(),
            target: "C:/Apps/mail.exe".into(),
        }];
        assert_eq!(
            filtered_app_indices(&refreshed, "EDITOR"),
            Vec::<usize>::new()
        );
        assert_eq!(filtered_app_indices(&refreshed, "mail"), vec![0]);
    }
}
