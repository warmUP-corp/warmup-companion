//! Best-effort suppression for Windows' built-in touch keyboard/input panel.
//!
//! The sign-in PIN field can ask Windows to show its own keyboard when focus is
//! retargeted. Warmup owns the visible VK, so hide any native panel windows that
//! appear on the current desktop.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_DWORD, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
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

/// Windows mirrors applied MDM `TextInput/*` policies (Policy CSP) under this
/// key; the touch keyboard stack reads them from here. They are device-scoped,
/// so unlike the per-profile `TabletTip` values they also govern the touch
/// keyboard LogonUI hosts on the secure desktop. The sign-in "Gamepad keyboard"
/// added in 26100.4762 (KB5062660) is that same touch keyboard's controller
/// layout, which `TouchKeyboardControllerModeAvailability` switches off by
/// contract instead of by racing its window. Overridden only while Companion
/// owns the secure desktop; restored on return to userland.
const TEXT_INPUT_POLICY_SUBKEY: &str = r"SOFTWARE\Microsoft\PolicyManager\current\device\TextInput";
const TEXT_INPUT_POLICY_VALUES: &[(&str, u32)] = &[
    // 2 = "Controller keyboard is always disabled".
    ("TouchKeyboardControllerModeAvailability", 2),
    // 0 = "Never" auto-invoke on edit-control focus, regardless of hardware.
    ("EnableTouchKeyboardAutoInvokeInDesktopMode", 0),
    // 0 = touch/handwriting keyboard not allowed.
    ("AllowInputPanel", 0),
];

/// Priors persisted here so a service that dies or restarts while on the
/// secure desktop can still put every value back. Without it a crash would
/// strand the device-wide policies above (no touch keyboard in userland).
const OVERRIDES_FILE: &str = "signin-overrides.json";

/// `Some(priors)` while we have registry values overridden. Each prior is the
/// value to restore (`None` = absent, delete it).
static AUTO_INVOKE_SAVED: Mutex<Option<Vec<RegPrior>>> = Mutex::new(None);
static TEXT_INPUT_SERVICES_SAVED: Mutex<Option<Vec<ServicePrior>>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RegPrior {
    root: TipRoot,
    subkey: String,
    value: String,
    prior: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ServicePrior {
    name: String,
    prior: Option<u32>,
}

/// Everything Companion changed for sign-in ownership, as written to
/// [`OVERRIDES_FILE`] while the overrides are live.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SavedOverrides {
    #[serde(default)]
    registry: Vec<RegPrior>,
    #[serde(default)]
    services: Vec<ServicePrior>,
}

impl SavedOverrides {
    /// The pre-Companion value recorded for a registry override, if this
    /// record has one. `Some(None)` means "was absent"; `None` means unknown.
    fn registry_prior(&self, root: TipRoot, subkey: &str, value: &str) -> Option<Option<u32>> {
        self.registry
            .iter()
            .find(|p| p.root == root && p.subkey == subkey && p.value == value)
            .map(|p| p.prior)
    }

    fn service_prior(&self, name: &str) -> Option<Option<u32>> {
        self.services
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.prior)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum TipRoot {
    Users,
    CurrentUser,
    LocalMachine,
}

impl TipRoot {
    fn hkey(self) -> HKEY {
        match self {
            Self::Users => HKEY_USERS,
            Self::CurrentUser => HKEY_CURRENT_USER,
            Self::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Users => "HKU",
            Self::CurrentUser => "HKCU",
            Self::LocalMachine => "HKLM",
        }
    }
}

fn overrides_path() -> PathBuf {
    Path::new(crate::install::DATA_DIR).join(OVERRIDES_FILE)
}

fn load_saved_overrides() -> Option<SavedOverrides> {
    let text = std::fs::read_to_string(overrides_path()).ok()?;
    match serde_json::from_str(&text) {
        Ok(saved) => Some(saved),
        Err(e) => {
            crate::install::log_line(&format!(
                "native kbd: ignoring unreadable {OVERRIDES_FILE}: {e}"
            ));
            None
        }
    }
}

fn store_saved_overrides(saved: &SavedOverrides) {
    let path = overrides_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let result = serde_json::to_string_pretty(saved)
        .map_err(std::io::Error::other)
        .and_then(|json| std::fs::write(&path, json));
    if let Err(e) = result {
        crate::install::log_line(&format!(
            "native kbd: could not persist {OVERRIDES_FILE}: {e} (restore after a crash will need `restore-keyboard`)"
        ));
    }
}

fn clear_saved_overrides() {
    let path = overrides_path();
    if path.is_file() {
        let _ = std::fs::remove_file(path);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Take ownership of text input on the secure desktop: turn off the touch
/// keyboard's controller mode and auto-invoke through the documented device
/// policies, the logon profile's `TabletTip` values, and the legacy panel
/// service, saving every prior value for [`restore_auto_invoke`]. Idempotent:
/// once overridden, repeated calls are no-ops (the priors stay captured).
pub fn disable_auto_invoke() {
    let Ok(mut saved) = AUTO_INVOKE_SAVED.lock() else {
        return;
    };
    if saved.is_some() {
        return;
    }
    // A previous process may have died with overrides live. Its record holds
    // the real pre-Companion values; the registry currently holds ours.
    let orphan = load_saved_overrides();
    if orphan.is_some() {
        crate::install::log_line(
            "native kbd: reusing priors recorded by a previous run (overrides were left live)",
        );
    }
    let mut priors = Vec::new();
    override_registry_values(
        TipRoot::Users,
        DEFAULT_TIP_SUBKEY,
        TIP_VALUES,
        orphan.as_ref(),
        &mut priors,
    );
    override_registry_values(
        TipRoot::CurrentUser,
        USER_TIP_SUBKEY,
        TIP_VALUES,
        orphan.as_ref(),
        &mut priors,
    );
    override_registry_values(
        TipRoot::LocalMachine,
        TEXT_INPUT_POLICY_SUBKEY,
        TEXT_INPUT_POLICY_VALUES,
        orphan.as_ref(),
        &mut priors,
    );
    if !priors.is_empty() {
        *saved = Some(priors.clone());
    }
    let services = disable_text_input_services(orphan.as_ref());
    store_saved_overrides(&SavedOverrides {
        registry: priors,
        services,
    });
    crate::install::log_line(&signin_ownership_report());
}

/// Restore every value saved by [`disable_auto_invoke`] (delete it if it was
/// originally absent). Falls back to the on-disk record when this process has
/// nothing in memory, so a fresh process (service restart after a crash, or the
/// `restore-keyboard` command) can undo what an earlier one changed. No-op if
/// neither exists.
pub fn restore_auto_invoke() {
    let Ok(mut saved) = AUTO_INVOKE_SAVED.lock() else {
        return;
    };
    let orphan = load_saved_overrides();
    let restored_services = restore_text_input_services(orphan.as_ref());

    let priors = match saved.take() {
        Some(p) => p,
        None => orphan.map(|o| o.registry).unwrap_or_default(),
    };
    let restored_registry = !priors.is_empty();
    for p in &priors {
        restore_registry_value(p);
    }
    clear_saved_overrides();
    if restored_registry || restored_services {
        crate::install::log_line(&format!(
            "native kbd: restored {} registry value(s) and {} service(s) for Windows text input",
            priors.len(),
            usize::from(restored_services)
        ));
    }
}

/// Startup hook for the userland side: if a previous run left sign-in
/// overrides live (crash, kill, power loss), put them back now rather than
/// waiting for a Winlogon round-trip that may never come.
pub fn restore_orphaned_overrides() {
    if !overrides_path().is_file() {
        return;
    }
    crate::install::log_line(
        "native kbd: sign-in overrides left by a previous run; restoring Windows text input",
    );
    restore_auto_invoke();
}

fn override_registry_values(
    root: TipRoot,
    subkey: &'static str,
    values: &[(&'static str, u32)],
    orphan: Option<&SavedOverrides>,
    priors: &mut Vec<RegPrior>,
) {
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
                "native kbd: key open failed {}\\{subkey} rc={}",
                root.label(),
                rc.0
            ));
            return;
        }

        for &(value_name, desired) in values {
            let value_w = wide(value_name);
            let prior = orphan
                .and_then(|o| o.registry_prior(root, subkey, value_name))
                .unwrap_or_else(|| read_dword(hkey, &value_w));
            let desired_bytes = desired.to_le_bytes();
            let rc = RegSetValueExW(
                hkey,
                PCWSTR(value_w.as_ptr()),
                0,
                REG_DWORD,
                Some(&desired_bytes),
            );
            if rc.0 == 0 {
                priors.push(RegPrior {
                    root,
                    subkey: subkey.to_string(),
                    value: value_name.to_string(),
                    prior,
                });
                crate::install::log_line(&format!(
                    "native kbd: set {}\\{subkey}\\{value_name}={desired} (prior={prior:?})",
                    root.label()
                ));
            } else {
                crate::install::log_line(&format!(
                    "native kbd: set failed {}\\{subkey}\\{value_name} rc={}",
                    root.label(),
                    rc.0
                ));
            }
        }
        let _ = RegCloseKey(hkey);
    }
}

fn restore_registry_value(p: &RegPrior) {
    unsafe {
        let subkey_w = wide(&p.subkey);
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
        let value_w = wide(&p.value);
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

/// Returns the priors now live (also kept in memory), for the on-disk record.
fn disable_text_input_services(orphan: Option<&SavedOverrides>) -> Vec<ServicePrior> {
    let Ok(mut saved) = TEXT_INPUT_SERVICES_SAVED.lock() else {
        return Vec::new();
    };
    if let Some(existing) = saved.as_ref() {
        return existing.clone();
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
            let prior = orphan
                .and_then(|o| o.service_prior(service_name))
                .unwrap_or_else(|| read_dword(hkey, &value));
            let disabled = DISABLED_SERVICE_START.to_le_bytes();
            let rc = RegSetValueExW(hkey, PCWSTR(value.as_ptr()), 0, REG_DWORD, Some(&disabled));
            if rc.0 == 0 {
                crate::install::log_line(&format!(
                    "native kbd: disabled text input service {service_name} (prior={prior:?})"
                ));
                stop_text_input_service(service_name);
                prior_values.push(ServicePrior {
                    name: service_name.to_string(),
                    prior,
                });
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
        *saved = Some(prior_values.clone());
    }
    prior_values
}

/// Returns true if anything was restored.
fn restore_text_input_services(orphan: Option<&SavedOverrides>) -> bool {
    let Ok(mut saved) = TEXT_INPUT_SERVICES_SAVED.lock() else {
        return false;
    };
    let prior_values = match saved.take() {
        Some(p) => p,
        None => orphan.map(|o| o.services.clone()).unwrap_or_default(),
    };
    if prior_values.is_empty() {
        return false;
    }

    for ServicePrior {
        name: service_name,
        prior,
    } in prior_values
    {
        unsafe {
            let subkey = service_registry_subkey(&service_name);
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
    true
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

static NATIVE_LOGON_VK: OnceLock<bool> = OnceLock::new();

static LOGON_XBOX_PAD: AtomicBool = AtomicBool::new(false);

pub fn set_logon_pad_is_xbox(xbox: bool) {
    LOGON_XBOX_PAD.store(xbox, Ordering::SeqCst);
}

pub fn logon_pad_is_xbox() -> bool {
    LOGON_XBOX_PAD.load(Ordering::SeqCst)
}

static LOGON_SIGNIN_SURFACE: AtomicBool = AtomicBool::new(false);

pub fn set_logon_signin_surface(signin: bool) {
    LOGON_SIGNIN_SURFACE.store(signin, Ordering::SeqCst);
}

pub fn window_is_logonui(hwnd: HWND) -> bool {
    window_process_image(hwnd)
        .as_deref()
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .is_some_and(|image| image.eq_ignore_ascii_case("LogonUI.exe"))
}

pub fn yield_logon_to_native() -> bool {
    native_logon_keyboard_available() && LOGON_SIGNIN_SURFACE.load(Ordering::SeqCst)
}

fn native_logon_requested(value: Option<&str>) -> bool {
    // An OS build number proves neither that the PIN panel is visible nor that
    // it supports the selected credential (e.g. a password). Keep L3 and our
    // keyboard available unless the native-only path is explicitly requested.
    value == Some("1")
}

pub fn native_logon_keyboard_available() -> bool {
    *NATIVE_LOGON_VK.get_or_init(|| {
        let value = std::env::var("WARMUP_NATIVE_LOGON_VK").ok();
        let native = native_logon_requested(value.as_deref());
        crate::install::log_line(&format!(
            "native kbd: native-only sign-in={native} (WARMUP_NATIVE_LOGON_VK={value:?})"
        ));
        native
    })
}

/// One override the ownership check reads back.
struct OwnershipEntry {
    root: TipRoot,
    value: &'static str,
    desired: u32,
    current: Option<u32>,
}

/// Read every sign-in override back from the registry and count native
/// keyboard windows currently visible on this desktop. One line, logged right
/// after the overrides are applied and printed by `signin-report`, so a machine
/// where the guarantee does not hold says so instead of flashing a panel.
pub fn signin_ownership_report() -> String {
    let mut entries = Vec::new();
    for (root, subkey, values) in [
        (TipRoot::Users, DEFAULT_TIP_SUBKEY, TIP_VALUES),
        (
            TipRoot::LocalMachine,
            TEXT_INPUT_POLICY_SUBKEY,
            TEXT_INPUT_POLICY_VALUES,
        ),
    ] {
        for &(value, desired) in values {
            entries.push(OwnershipEntry {
                root,
                value,
                desired,
                current: unsafe { read_dword_at(root, subkey, value) },
            });
        }
    }
    format_ownership_report(&entries, count_native_keyboard_windows())
}

fn format_ownership_report(entries: &[OwnershipEntry], native_windows_visible: usize) -> String {
    let mut missing = 0usize;
    let mut parts = Vec::with_capacity(entries.len());
    for e in entries {
        let applied = e.current == Some(e.desired);
        if !applied {
            missing += 1;
        }
        parts.push(format!(
            "{}\\{}={} {}",
            e.root.label(),
            e.value,
            e.desired,
            if applied {
                "ok".to_string()
            } else {
                format!(
                    "MISSING(current={})",
                    e.current.map_or("absent".to_string(), |v| v.to_string())
                )
            }
        ));
    }
    let verdict = if missing == 0 && native_windows_visible == 0 {
        "OWNED".to_string()
    } else {
        format!(
            "NOT GUARANTEED ({missing} override(s) missing, {native_windows_visible} native keyboard window(s) visible)"
        )
    };
    format!(
        "sign-in ownership: {verdict}; {}; native keyboard windows visible={native_windows_visible}",
        parts.join(", ")
    )
}

unsafe fn read_dword_at(root: TipRoot, subkey: &str, value: &str) -> Option<u32> {
    let subkey_w = wide(subkey);
    let mut hkey = HKEY::default();
    let rc = RegOpenKeyExW(
        root.hkey(),
        PCWSTR(subkey_w.as_ptr()),
        0,
        KEY_QUERY_VALUE,
        &mut hkey,
    );
    if rc.0 != 0 {
        return None;
    }
    let result = read_dword(hkey, &wide(value));
    let _ = RegCloseKey(hkey);
    result
}

/// Visible native keyboard windows on the current desktop, without touching
/// them. `suppress()` is the acting counterpart.
fn count_native_keyboard_windows() -> usize {
    let mut count = 0usize;
    unsafe {
        let _ = EnumWindows(
            Some(enum_count_native),
            LPARAM(&mut count as *mut usize as isize),
        );
    }
    count
}

unsafe extern "system" fn enum_count_native(hwnd: HWND, param: LPARAM) -> BOOL {
    if IsWindowVisible(hwnd).as_bool() {
        let class = window_class(hwnd);
        let title = window_title(hwnd);
        let process = window_process_image(hwnd);
        if is_native_keyboard_window(&class, &title, process.as_deref()) {
            let count = param.0 as *mut usize;
            if !count.is_null() {
                *count += 1;
            }
        }
    }
    true.into()
}

pub fn suppress() {
    // Every caller (including UIA focus and post-injection sweeps) must honor
    // the same owner. Otherwise native PIN injection hides its own keyboard.
    if yield_logon_to_native() {
        return;
    }
    unsafe {
        let _ = EnumWindows(Some(enum_window), LPARAM(0));
    }
}

pub fn suppress_for(duration: Duration) {
    if yield_logon_to_native() {
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_signin_is_default_and_native_requires_explicit_opt_in() {
        for value in [None, Some("0"), Some(""), Some("false"), Some("typo")] {
            assert!(!native_logon_requested(value), "{value:?}");
        }
        assert!(native_logon_requested(Some("1")));
    }

    #[test]
    fn controller_keyboard_policy_is_part_of_signin_ownership() {
        let policy = |name: &str| {
            TEXT_INPUT_POLICY_VALUES
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
        };
        // Policy CSP TextInput: 2 = controller keyboard always disabled,
        // 0 = never auto-invoke, 0 = input panel not allowed.
        assert_eq!(policy("TouchKeyboardControllerModeAvailability"), Some(2));
        assert_eq!(
            policy("EnableTouchKeyboardAutoInvokeInDesktopMode"),
            Some(0)
        );
        assert_eq!(policy("AllowInputPanel"), Some(0));
        assert!(TEXT_INPUT_POLICY_SUBKEY
            .starts_with(r"SOFTWARE\Microsoft\PolicyManager\current\device\"));
    }

    #[test]
    fn saved_overrides_round_trip_and_resolve_recorded_priors() {
        let saved = SavedOverrides {
            registry: vec![
                RegPrior {
                    root: TipRoot::LocalMachine,
                    subkey: TEXT_INPUT_POLICY_SUBKEY.into(),
                    value: "AllowInputPanel".into(),
                    prior: None,
                },
                RegPrior {
                    root: TipRoot::Users,
                    subkey: DEFAULT_TIP_SUBKEY.into(),
                    value: "EnableDesktopModeAutoInvoke".into(),
                    prior: Some(1),
                },
            ],
            services: vec![ServicePrior {
                name: "TabletInputService".into(),
                prior: Some(3),
            }],
        };
        let json = serde_json::to_string(&saved).unwrap();
        let back: SavedOverrides = serde_json::from_str(&json).unwrap();
        assert_eq!(back, saved);

        // "Was absent" must survive as Some(None), distinct from "unknown".
        assert_eq!(
            back.registry_prior(
                TipRoot::LocalMachine,
                TEXT_INPUT_POLICY_SUBKEY,
                "AllowInputPanel"
            ),
            Some(None)
        );
        assert_eq!(
            back.registry_prior(
                TipRoot::Users,
                DEFAULT_TIP_SUBKEY,
                "EnableDesktopModeAutoInvoke"
            ),
            Some(Some(1))
        );
        assert_eq!(
            back.registry_prior(
                TipRoot::CurrentUser,
                USER_TIP_SUBKEY,
                "EnableDesktopModeAutoInvoke"
            ),
            None
        );
        assert_eq!(back.service_prior("TabletInputService"), Some(Some(3)));
        assert_eq!(back.service_prior("TextInputManagementService"), None);

        // Older or hand-edited records without one of the sections still load.
        let partial: SavedOverrides = serde_json::from_str(r#"{"registry":[]}"#).unwrap();
        assert_eq!(partial, SavedOverrides::default());
    }

    #[test]
    fn ownership_report_is_owned_only_when_everything_applied_and_nothing_visible() {
        let entry = |value: &'static str, desired, current| OwnershipEntry {
            root: TipRoot::LocalMachine,
            value,
            desired,
            current,
        };
        let all_ok = [
            entry("TouchKeyboardControllerModeAvailability", 2, Some(2)),
            entry("AllowInputPanel", 0, Some(0)),
        ];
        let report = format_ownership_report(&all_ok, 0);
        assert!(report.starts_with("sign-in ownership: OWNED;"), "{report}");
        assert!(report.contains("HKLM\\TouchKeyboardControllerModeAvailability=2 ok"));

        let report = format_ownership_report(&all_ok, 1);
        assert!(
            report.contains(
                "NOT GUARANTEED (0 override(s) missing, 1 native keyboard window(s) visible)"
            ),
            "{report}"
        );

        let drifted = [
            entry("TouchKeyboardControllerModeAvailability", 2, Some(0)),
            entry("AllowInputPanel", 0, None),
        ];
        let report = format_ownership_report(&drifted, 0);
        assert!(
            report.contains("NOT GUARANTEED (2 override(s) missing"),
            "{report}"
        );
        assert!(report.contains("TouchKeyboardControllerModeAvailability=2 MISSING(current=0)"));
        assert!(report.contains("AllowInputPanel=0 MISSING(current=absent)"));
    }
}
