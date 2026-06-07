//! Best-effort suppression for Windows' built-in touch keyboard/input panel.
//!
//! The sign-in PIN field can ask Windows to show its own keyboard when focus is
//! retargeted. Warmup owns the visible VK, so hide any native panel windows that
//! appear on the current desktop.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD,
    REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, ShowWindow, SW_HIDE, WM_CLOSE,
};

static SUPPRESSING: AtomicBool = AtomicBool::new(false);

/// Touch-keyboard auto-invoke registry control. The hide-after-show loop below
/// catches the panel (`TextInputHost` / `TabTip`) but loses the race — it
/// re-shows faster than the 25 ms sweep, so it flashes. Disabling auto-invoke
/// stops it being summoned on field focus in the first place.
///
/// LogonUI runs as SYSTEM, whose `HKCU` is `HKEY_USERS\.DEFAULT`, so the sign-in
/// touch keyboard reads its setting from there. `0` = no auto-invoke.
const DEFAULT_TIP_SUBKEY: &str = ".DEFAULT\\Software\\Microsoft\\TabletTip\\1.7";
const USER_TIP_SUBKEY: &str = "Software\\Microsoft\\TabletTip\\1.7";
const TIP_VALUES: &[(&str, u32)] = &[
    ("TouchKeyboardTapInvoke", 0),
    ("EnableDesktopModeAutoInvoke", 0),
    ("DisableNewKeyboardExperience", 1),
];
const SERVICE_START_VALUE: &str = "Start";
const DISABLED_SERVICE_START: u32 = 4;
/// `TextInputManagementService` also powers userland Start-menu / taskbar search,
/// and its DACL denies `SERVICE_CHANGE_CONFIG` to everyone — so it must NOT be
/// disabled via the registry `Start` value (that strands search until a reboot,
/// unrecoverable live). It is toggled by live stop/start instead — see
/// [`stop_search_service`] / [`ensure_search_service_running`]. Keep it OUT of
/// the registry-disable list below.
const SEARCH_SERVICE: &str = "TextInputManagementService";
// Registry `Start`-toggled services. TextInputManagementService is deliberately
// NOT here (see SEARCH_SERVICE) — it is toggled by live stop/start instead.
const TEXT_INPUT_SERVICES: &[&str] = &["TabletInputService"];

/// `Some(priors)` while we have TabletTip values overridden. Each prior is the
/// value to restore (`None` = absent, delete it).
static AUTO_INVOKE_SAVED: Mutex<Option<Vec<TipPrior>>> = Mutex::new(None);
static TEXT_INPUT_SERVICES_SAVED: Mutex<Option<Vec<(&'static str, Option<u32>)>>> =
    Mutex::new(None);

#[derive(Clone, Copy)]
struct TipPrior {
    root: TipRoot,
    subkey: &'static str,
    value: &'static str,
    prior: Option<u32>,
}

#[derive(Clone, Copy)]
enum TipRoot {
    Users,
    CurrentUser,
}

impl TipRoot {
    fn hkey(self) -> HKEY {
        match self {
            Self::Users => HKEY_USERS,
            Self::CurrentUser => HKEY_CURRENT_USER,
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Disable the native touch keyboard's auto-invoke on the secure-desktop logon
/// profile, saving the prior value for [`restore_auto_invoke`]. Idempotent:
/// once overridden, repeated calls are no-ops (the prior value stays captured).
pub fn disable_auto_invoke() {
    let Ok(mut saved) = AUTO_INVOKE_SAVED.lock() else {
        return;
    };
    if saved.is_some() {
        return;
    }
    let mut priors = Vec::new();
    override_tablet_tip_values(TipRoot::Users, DEFAULT_TIP_SUBKEY, &mut priors);
    override_tablet_tip_values(TipRoot::CurrentUser, USER_TIP_SUBKEY, &mut priors);
    if !priors.is_empty() {
        *saved = Some(priors);
    }
    disable_text_input_services();
}

/// Restore the auto-invoke value saved by [`disable_auto_invoke`] (delete it if
/// it was originally absent). No-op if we never overrode it.
pub fn restore_auto_invoke() {
    restore_text_input_services();

    let Ok(mut saved) = AUTO_INVOKE_SAVED.lock() else {
        return;
    };
    let Some(priors) = saved.take() else {
        return;
    };

    for p in priors {
        restore_tablet_tip_value(p);
    }
    crate::install::log_line("native kbd: restored TabletTip keyboard values");
}

fn override_tablet_tip_values(root: TipRoot, subkey: &'static str, priors: &mut Vec<TipPrior>) {
    unsafe {
        let subkey_w = wide(subkey);
        let mut hkey = HKEY::default();
        let rc = RegCreateKeyExW(
            root.hkey(),
            PCWSTR(subkey_w.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if rc.0 != 0 {
            crate::install::log_line(&format!(
                "native kbd: TabletTip key open failed {subkey} rc={}",
                rc.0
            ));
            return;
        }

        for &(value_name, desired) in TIP_VALUES {
            let value_w = wide(value_name);
            let prior = read_dword(hkey, &value_w);
            let desired_bytes = desired.to_le_bytes();
            let rc = RegSetValueExW(
                hkey,
                PCWSTR(value_w.as_ptr()),
                0,
                REG_DWORD,
                Some(&desired_bytes),
            );
            if rc.0 == 0 {
                priors.push(TipPrior {
                    root,
                    subkey,
                    value: value_name,
                    prior,
                });
                crate::install::log_line(&format!(
                    "native kbd: set TabletTip {subkey}\\{value_name}={desired} (prior={prior:?})"
                ));
            } else {
                crate::install::log_line(&format!(
                    "native kbd: TabletTip set failed {subkey}\\{value_name} rc={}",
                    rc.0
                ));
            }
        }
        let _ = RegCloseKey(hkey);
    }
}

fn restore_tablet_tip_value(p: TipPrior) {
    unsafe {
        let subkey_w = wide(p.subkey);
        let mut hkey = HKEY::default();
        let rc = RegCreateKeyExW(
            p.root.hkey(),
            PCWSTR(subkey_w.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if rc.0 != 0 {
            return;
        }
        let value_w = wide(p.value);
        match p.prior {
            Some(v) => {
                let _ = RegSetValueExW(
                    hkey,
                    PCWSTR(value_w.as_ptr()),
                    0,
                    REG_DWORD,
                    Some(&v.to_le_bytes()),
                );
            }
            None => {
                let _ = RegDeleteValueW(hkey, PCWSTR(value_w.as_ptr()));
            }
        }
        let _ = RegCloseKey(hkey);
    }
}

fn disable_text_input_services() {
    let Ok(mut saved) = TEXT_INPUT_SERVICES_SAVED.lock() else {
        return;
    };
    if saved.is_some() {
        return;
    }

    let mut prior_values = Vec::new();
    for &service_name in TEXT_INPUT_SERVICES {
        unsafe {
            let subkey = service_registry_subkey(service_name);
            let subkey_w = wide(&subkey);
            let mut hkey = HKEY::default();
            let rc = RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(subkey_w.as_ptr()),
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            );
            if rc.0 != 0 {
                crate::install::log_line(&format!(
                    "native kbd: service key open failed {service_name} rc={}",
                    rc.0
                ));
                continue;
            }

            let value = wide(SERVICE_START_VALUE);
            let prior = read_dword(hkey, &value);
            let disabled = DISABLED_SERVICE_START.to_le_bytes();
            let rc = RegSetValueExW(hkey, PCWSTR(value.as_ptr()), 0, REG_DWORD, Some(&disabled));
            if rc.0 == 0 {
                crate::install::log_line(&format!(
                    "native kbd: disabled text input service {service_name} (prior={prior:?})"
                ));
                stop_text_input_service(service_name);
                prior_values.push((service_name, prior));
            } else {
                crate::install::log_line(&format!(
                    "native kbd: service disable failed {service_name} rc={}",
                    rc.0
                ));
            }
            let _ = RegCloseKey(hkey);
        }
    }

    if !prior_values.is_empty() {
        *saved = Some(prior_values);
    }
}

fn restore_text_input_services() {
    let Ok(mut saved) = TEXT_INPUT_SERVICES_SAVED.lock() else {
        return;
    };
    let Some(prior_values) = saved.take() else {
        return;
    };

    for (service_name, prior) in prior_values {
        unsafe {
            let subkey = service_registry_subkey(service_name);
            let subkey_w = wide(&subkey);
            let mut hkey = HKEY::default();
            let rc = RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(subkey_w.as_ptr()),
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            );
            if rc.0 != 0 {
                continue;
            }

            let value = wide(SERVICE_START_VALUE);
            match prior {
                Some(v) => {
                    let _ = RegSetValueExW(
                        hkey,
                        PCWSTR(value.as_ptr()),
                        0,
                        REG_DWORD,
                        Some(&v.to_le_bytes()),
                    );
                }
                None => {
                    let _ = RegDeleteValueW(hkey, PCWSTR(value.as_ptr()));
                }
            }
            crate::install::log_line(&format!(
                "native kbd: restored text input service {service_name}"
            ));
            let _ = RegCloseKey(hkey);
        }
    }
}

fn service_registry_subkey(service_name: &str) -> String {
    format!(r"SYSTEM\CurrentControlSet\Services\{service_name}")
}

fn stop_text_input_service(service_name: &'static str) {
    let name = service_name.to_string();
    if thread::Builder::new()
        .name(format!("warmup-stop-{service_name}"))
        .spawn(move || {
            let output = hidden_command(Path::new("sc.exe"))
                .args(["stop", name.as_str()])
                .output();
            match output {
                Ok(out) if out.status.success() => {
                    crate::install::log_line(&format!("native kbd: stopped service {name}"));
                }
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    crate::install::log_line(&format!(
                        "native kbd: stop service {name} returned {} stdout='{}' stderr='{}'",
                        out.status,
                        stdout.trim(),
                        stderr.trim()
                    ));
                }
                Err(e) => {
                    crate::install::log_line(&format!(
                        "native kbd: stop service {name} spawn failed: {e}"
                    ));
                }
            }
        })
        .is_err()
    {
        crate::install::log_line(&format!(
            "native kbd: stop service {service_name} thread spawn failed"
        ));
    }
}

/// Stop [`SEARCH_SERVICE`] live for the secure desktop, suppressing the gamepad
/// keyboard. We toggle the *running state*, never the `Start` type: the service
/// DACL denies `SERVICE_CHANGE_CONFIG` to everyone (so `sc config` fails with
/// access-denied 5), and a raw-registry `Start=4` write disables it in the SCM
/// at next boot with no way to re-enable it live (`sc start` then fails 1058).
/// LocalSystem — our service account — holds `SERVICE_STOP` per the DACL, so a
/// live stop works while `Start` stays `2` (auto). See [`ensure_search_service_running`].
pub fn stop_search_service() {
    spawn_sc("stop", SEARCH_SERVICE);
}

/// Start [`SEARCH_SERVICE`] live so userland Start-menu / taskbar search works.
/// Idempotent: a benign 1056 (already running) is treated as success. Because we
/// never change the `Start` type, the service is always enabled in the SCM and
/// LocalSystem's `SERVICE_START` right lets this succeed in the live session —
/// and any reboot autostarts it (`Start=2`), so search can never be stranded.
pub fn ensure_search_service_running() {
    spawn_sc("start", SEARCH_SERVICE);
}

/// Run `sc.exe <action> <service>` off-thread (SCM calls can block). Logs the
/// outcome; non-zero exit codes are logged but not treated as fatal (e.g. 1056
/// = already running, 1062 = not started — both benign for our idempotent use).
fn spawn_sc(action: &'static str, service_name: &'static str) {
    let name = service_name.to_string();
    if thread::Builder::new()
        .name(format!("warmup-sc-{action}-{service_name}"))
        .spawn(move || {
            match hidden_command(Path::new("sc.exe"))
                .args([action, name.as_str()])
                .output()
            {
                Ok(out) if out.status.success() => {
                    crate::install::log_line(&format!("native kbd: sc {action} {name} ok"));
                }
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    crate::install::log_line(&format!(
                        "native kbd: sc {action} {name} returned {} stdout='{}' stderr='{}'",
                        out.status,
                        stdout.trim(),
                        stderr.trim()
                    ));
                }
                Err(e) => {
                    crate::install::log_line(&format!(
                        "native kbd: sc {action} {name} spawn failed: {e}"
                    ));
                }
            }
        })
        .is_err()
    {
        crate::install::log_line(&format!(
            "native kbd: sc {action} {service_name} thread spawn failed"
        ));
    }
}

fn hidden_command(exe: &Path) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new(exe);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        Command::new(exe)
    }
}

unsafe fn read_dword(hkey: HKEY, value_name: &[u16]) -> Option<u32> {
    let mut ty = REG_VALUE_TYPE::default();
    let mut data = [0u8; 4];
    let mut len = data.len() as u32;
    let rc = RegQueryValueExW(
        hkey,
        PCWSTR(value_name.as_ptr()),
        None,
        Some(&mut ty),
        Some(data.as_mut_ptr()),
        Some(&mut len),
    );
    if rc.0 == 0 && len == 4 {
        Some(u32::from_le_bytes(data))
    } else {
        None
    }
}

pub fn suppress() {
    unsafe {
        let _ = EnumWindows(Some(enum_window), LPARAM(0));
    }
}

pub fn suppress_for(duration: Duration) {
    if SUPPRESSING.swap(true, Ordering::SeqCst) {
        return;
    }
    if thread::Builder::new()
        .name("warmup-native-keyboard-suppress".into())
        .spawn(move || {
            let _ = super::desktop::attach_input();
            let deadline = Instant::now() + duration;
            while Instant::now() < deadline {
                suppress();
                thread::sleep(Duration::from_millis(25));
            }
            suppress();
            SUPPRESSING.store(false, Ordering::SeqCst);
        })
        .is_err()
    {
        SUPPRESSING.store(false, Ordering::SeqCst);
    }
}

unsafe extern "system" fn enum_window(hwnd: HWND, _param: LPARAM) -> BOOL {
    if !IsWindowVisible(hwnd).as_bool() {
        return true.into();
    }

    let class = window_class(hwnd);
    let title = window_title(hwnd);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    let process = window_process_image(hwnd);
    let image = process
        .as_deref()
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .unwrap_or_default()
        .to_string();
    let on_winlogon = crate::win::logon_focus::is_active();
    if is_native_keyboard_window(&class, &title, process.as_deref()) {
        crate::install::log_line(&format!(
            "native keyboard suppress: class='{class}' title='{title}' process='{image}' winlogon={on_winlogon}"
        ));
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        // Hide+close doesn't stick: TextInputHost re-shows faster than the sweep,
        // and once the shell's gamepad text-input is armed (after an XInput pad)
        // it keeps summoning it. On the secure desktop, kill the host outright —
        // it's not needed for PIN entry there and the OS relaunches it later.
        if on_winlogon && is_killable_keyboard_image(&image) {
            terminate_pid(pid);
        }
    } else if on_winlogon && looks_input_related(&class, &image) {
        // Diagnostic: a popup the predicate missed. Log its identity so the match
        // list / kill list can be widened to whatever the shell actually spawns.
        crate::install::log_line(&format!(
            "native kbd seen (unmatched): class='{class}' title='{title}' process='{image}'"
        ));
    }
    true.into()
}

/// Process images safe to terminate on the secure desktop to kill the touch
/// keyboard. PIN entry there uses physical keys / mouse clicks, not these.
fn is_killable_keyboard_image(image: &str) -> bool {
    image.eq_ignore_ascii_case("TextInputHost.exe")
        || image.eq_ignore_ascii_case("TabTip.exe")
        || image.eq_ignore_ascii_case("osk.exe")
}

/// Loose net for the diagnostic branch: anything that smells like a text-input
/// surface, so a missed popup gets logged for identification.
fn looks_input_related(class: &str, image: &str) -> bool {
    is_killable_keyboard_image(image)
        || class.contains("IPTip")
        || class == "Windows.UI.Core.CoreWindow"
        || class == "ApplicationFrameWindow"
}

unsafe fn terminate_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
        let _ = TerminateProcess(handle, 1);
        let _ = CloseHandle(handle);
        crate::install::log_line(&format!(
            "native kbd: terminated touch-keyboard host pid={pid}"
        ));
    }
}

fn window_class(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 128];
        let n = GetClassNameW(hwnd, &mut buf);
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            String::new()
        }
    }
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            String::new()
        }
    }
}

fn window_process_image(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 32768];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result
            .ok()
            .map(|_| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn is_native_keyboard_window(class: &str, title: &str, process: Option<&str>) -> bool {
    let image = process
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .unwrap_or_default();
    class == "IPTip_Main_Window"
        || class == "IPTip_Window"
        || class == "ApplicationFrameWindow" && title == "Windows Input Experience"
        || image.eq_ignore_ascii_case("TextInputHost.exe")
        || image.eq_ignore_ascii_case("TabTip.exe")
        || image.eq_ignore_ascii_case("osk.exe")
        || (class == "Windows.UI.Core.CoreWindow"
            && (title == "Microsoft Text Input Application"
                || title == "Windows Input Experience"
                || title.contains("Text Input")))
}
