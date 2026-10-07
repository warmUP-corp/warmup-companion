use std::sync::Mutex;

use windows::core::{w, BSTR, HSTRING, PCWSTR, VARIANT};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, SetMonitorBrightness, PHYSICAL_MONITOR,
};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LUID};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoSetProxyBlanket, CLSCTX_ALL, CLSCTX_INPROC_SERVER, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, STGM_READ,
};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::RemoteDesktop::{
    WTSGetActiveConsoleSessionId, WTSLogoffSession, WTS_CURRENT_SERVER_HANDLE,
};
use windows::Win32::System::Shutdown::{
    ExitWindowsEx, LockWorkStation, EWX_POWEROFF, EWX_REBOOT, SHTDN_REASON_FLAG_PLANNED,
    SHTDN_REASON_MAJOR_OTHER,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, IWbemContext, IWbemLocator, IWbemServices, WbemLocator,
    WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE, WBEM_INFINITE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, VIRTUAL_KEY, VK_F4, VK_LWIN, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE,
    VK_MEDIA_PREV_TRACK, VK_MENU,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow, SW_SHOWNORMAL};

pub(crate) fn send_chord(keys: &[VIRTUAL_KEY]) {
    let mut batch: Vec<INPUT> = keys
        .iter()
        .map(|k| crate::vk_nav::vk_event(*k, false))
        .collect();
    batch.extend(keys.iter().rev().map(|k| crate::vk_nav::vk_event(*k, true)));
    unsafe {
        let _ = SendInput(&batch, std::mem::size_of::<INPUT>() as i32);
    }
}

pub(crate) fn win_chord(key: VIRTUAL_KEY) {
    send_chord(&[VK_LWIN, key]);
}

pub(crate) fn win_alt_chord(key: VIRTUAL_KEY) {
    send_chord(&[VK_LWIN, VK_MENU, key]);
}

pub(crate) fn close_window(target: HWND) {
    unsafe {
        if !target.is_invalid() && IsWindow(target).as_bool() {
            let _ = SetForegroundWindow(target);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
    send_chord(&[VK_MENU, VK_F4]);
}

pub(crate) fn open_sound_settings() {
    shell_open("ms-settings:sound");
}

fn shell_open(target: &str) {
    let target = HSTRING::from(target);
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            &target,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct NowPlaying {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) app: String,
    pub(crate) playing: bool,
}

static MEDIA: Mutex<Option<NowPlaying>> = Mutex::new(None);

pub(crate) fn app_name(aumid: &str) -> String {
    let tail = aumid.rsplit(['!', '\\', '/']).next().unwrap_or(aumid);
    let tail = tail.strip_suffix(".exe").unwrap_or(tail);
    let tail = tail.rsplit('.').next().unwrap_or(tail);
    let mut chars = tail.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn read_media() -> Option<NowPlaying> {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    let manager = Manager::RequestAsync().ok()?.get().ok()?;
    let session = manager.GetCurrentSession().ok()?;
    let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = props.Title().ok()?.to_string();
    if title.is_empty() {
        return None;
    }
    let playing = session
        .GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .map(|s| s == Status::Playing)
        .unwrap_or(false);
    Some(NowPlaying {
        title,
        artist: props.Artist().map(|a| a.to_string()).unwrap_or_default(),
        app: session
            .SourceAppUserModelId()
            .map(|a| app_name(&a.to_string()))
            .unwrap_or_default(),
        playing,
    })
}

pub(crate) fn refresh_media() {
    let _ = std::thread::Builder::new()
        .name("quick-media".into())
        .spawn(|| {
            let now = read_media();
            if let Ok(mut m) = MEDIA.lock() {
                *m = now;
            }
        });
}

pub(crate) fn now_playing() -> Option<NowPlaying> {
    MEDIA.lock().ok().and_then(|m| m.clone())
}

pub(crate) fn media_command(delta: i32) {
    let _ = std::thread::Builder::new()
        .name("quick-media-cmd".into())
        .spawn(move || {
            if !session_command(delta) {
                let vk = match delta {
                    d if d < 0 => VK_MEDIA_PREV_TRACK,
                    d if d > 0 => VK_MEDIA_NEXT_TRACK,
                    _ => VK_MEDIA_PLAY_PAUSE,
                };
                send_chord(&[vk]);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            let now = read_media();
            if let Ok(mut m) = MEDIA.lock() {
                *m = now;
            }
        });
}

fn session_command(delta: i32) -> bool {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;
    let Some(session) = Manager::RequestAsync()
        .ok()
        .and_then(|op| op.get().ok())
        .and_then(|m| m.GetCurrentSession().ok())
    else {
        return false;
    };
    let op = match delta {
        d if d < 0 => session.TrySkipPreviousAsync(),
        d if d > 0 => session.TrySkipNextAsync(),
        _ => session.TryTogglePlayPauseAsync(),
    };
    op.and_then(|op| op.get()).unwrap_or(false)
}

unsafe fn endpoint_volume() -> Option<IAudioEndpointVolume> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    dev.Activate(CLSCTX_ALL, None).ok()
}

pub(crate) fn volume() -> Option<(f32, bool)> {
    unsafe {
        let vol = endpoint_volume()?;
        let level = vol.GetMasterVolumeLevelScalar().ok()?;
        let muted = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        Some((level, muted))
    }
}

pub(crate) fn step_volume(level: f32, steps: i32) -> f32 {
    let pct = (level * 100.0).round() as i32 + steps * 2;
    pct.clamp(0, 100) as f32 / 100.0
}

pub(crate) fn set_volume(level: f32) {
    unsafe {
        if let Some(vol) = endpoint_volume() {
            let _ = vol.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null());
            if level > 0.0 {
                let _ = vol.SetMute(false, std::ptr::null());
            }
        }
    }
}

pub(crate) fn toggle_mute() {
    unsafe {
        if let Some(vol) = endpoint_volume() {
            let muted = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            let _ = vol.SetMute(!muted, std::ptr::null());
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Output {
    pub(crate) name: String,
    pub(crate) current: bool,
}

pub(crate) fn outputs() -> Vec<Output> {
    unsafe {
        let Ok(en) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return Vec::new();
        };
        let current = en
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .and_then(|d| d.GetId())
            .ok()
            .and_then(|id| {
                let s = id.to_string().ok();
                windows::Win32::System::Com::CoTaskMemFree(Some(id.0.cast()));
                s
            });
        let Ok(list) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
            return Vec::new();
        };
        let count = list.GetCount().unwrap_or(0);
        let mut out = Vec::new();
        for i in 0..count {
            let Ok(dev) = list.Item(i) else {
                continue;
            };
            let id = dev.GetId().ok().and_then(|id| {
                let s = id.to_string().ok();
                windows::Win32::System::Com::CoTaskMemFree(Some(id.0.cast()));
                s
            });
            let name = dev
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                .map(|v| v.to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            out.push(Output {
                name,
                current: id.is_some() && id == current,
            });
        }
        out
    }
}

pub(crate) fn short_output_name(name: &str) -> String {
    let base = name.split(" (").next().unwrap_or(name).trim();
    if base.is_empty() {
        name.to_string()
    } else {
        base.to_string()
    }
}

unsafe fn wmi() -> Option<IWbemServices> {
    let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
    let svc = loc
        .ConnectServer(
            &BSTR::from("ROOT\\WMI"),
            &BSTR::new(),
            &BSTR::new(),
            &BSTR::new(),
            0,
            &BSTR::new(),
            None::<&IWbemContext>,
        )
        .ok()?;
    let _ = CoSetProxyBlanket(
        &svc,
        10,
        0,
        PCWSTR::null(),
        RPC_C_AUTHN_LEVEL_CALL,
        RPC_C_IMP_LEVEL_IMPERSONATE,
        None,
        EOAC_NONE,
    );
    Some(svc)
}

unsafe fn wmi_first(svc: &IWbemServices, query: &str) -> Option<IWbemClassObject> {
    let en: IEnumWbemClassObject = svc
        .ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(query),
            WBEM_GENERIC_FLAG_TYPE(WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0),
            None::<&IWbemContext>,
        )
        .ok()?;
    let mut objs = [None];
    let mut n = 0u32;
    let _ = en.Next(WBEM_INFINITE, &mut objs, &mut n);
    objs[0].take()
}

fn wmi_brightness() -> Option<f32> {
    unsafe {
        let svc = wmi()?;
        let obj = wmi_first(&svc, "SELECT CurrentBrightness FROM WmiMonitorBrightness")?;
        let mut v = VARIANT::default();
        obj.Get(w!("CurrentBrightness"), 0, &mut v, None, None)
            .ok()?;
        let pct = u32::try_from(&v)
            .ok()
            .or_else(|| i32::try_from(&v).ok().map(|x| x.max(0) as u32))
            .or_else(|| u16::try_from(&v).ok().map(u32::from))?;
        Some(pct.min(100) as f32 / 100.0)
    }
}

fn wmi_set_brightness(level: f32) -> bool {
    unsafe {
        let Some(svc) = wmi() else {
            return false;
        };
        let Some(inst) = wmi_first(&svc, "SELECT * FROM WmiMonitorBrightnessMethods") else {
            return false;
        };
        let mut path = VARIANT::default();
        if inst.Get(w!("__PATH"), 0, &mut path, None, None).is_err() {
            return false;
        }
        let Ok(path) = BSTR::try_from(&path) else {
            return false;
        };
        let mut class: Option<IWbemClassObject> = None;
        if svc
            .GetObject(
                &BSTR::from("WmiMonitorBrightnessMethods"),
                WBEM_GENERIC_FLAG_TYPE(0),
                None::<&IWbemContext>,
                Some(&mut class),
                None,
            )
            .is_err()
        {
            return false;
        }
        let Some(class) = class else {
            return false;
        };
        let mut sig: Option<IWbemClassObject> = None;
        if class
            .GetMethod(w!("WmiSetBrightness"), 0, &mut sig, std::ptr::null_mut())
            .is_err()
        {
            return false;
        }
        let Some(params) = sig.and_then(|s| s.SpawnInstance(0).ok()) else {
            return false;
        };
        let pct = (level.clamp(0.0, 1.0) * 100.0).round() as u8;
        let _ = params.Put(w!("Timeout"), 0, &VARIANT::from(0i32), 0);
        let _ = params.Put(w!("Brightness"), 0, &VARIANT::from(pct as i32), 0);
        svc.ExecMethod(
            &path,
            &BSTR::from("WmiSetBrightness"),
            WBEM_GENERIC_FLAG_TYPE(0),
            None::<&IWbemContext>,
            &params,
            None,
            None,
        )
        .is_ok()
    }
}

fn with_physical_monitors<T>(
    near: HWND,
    f: impl FnOnce(&[PHYSICAL_MONITOR]) -> Option<T>,
) -> Option<T> {
    unsafe {
        let mon = MonitorFromWindow(near, MONITOR_DEFAULTTONEAREST);
        let mut count = 0u32;
        GetNumberOfPhysicalMonitorsFromHMONITOR(mon, &mut count).ok()?;
        if count == 0 {
            return None;
        }
        let mut list = vec![PHYSICAL_MONITOR::default(); count as usize];
        GetPhysicalMonitorsFromHMONITOR(mon, &mut list).ok()?;
        let out = f(&list);
        let _ = DestroyPhysicalMonitors(&list);
        out
    }
}

fn ddc_brightness(near: HWND) -> Option<f32> {
    with_physical_monitors(near, |list| unsafe {
        let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
        if GetMonitorBrightness(list[0].hPhysicalMonitor, &mut min, &mut cur, &mut max) == 0
            || max <= min
        {
            return None;
        }
        Some((cur.saturating_sub(min)) as f32 / (max - min) as f32)
    })
}

fn ddc_set_brightness(near: HWND, level: f32) -> bool {
    with_physical_monitors(near, |list| unsafe {
        let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
        if GetMonitorBrightness(list[0].hPhysicalMonitor, &mut min, &mut cur, &mut max) == 0
            || max <= min
        {
            return None;
        }
        let v = min + ((max - min) as f32 * level.clamp(0.0, 1.0)).round() as u32;
        Some(SetMonitorBrightness(list[0].hPhysicalMonitor, v) != 0)
    })
    .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrightnessSource {
    Wmi,
    Ddc,
}

pub(crate) fn brightness(near: HWND) -> Option<(f32, BrightnessSource)> {
    wmi_brightness()
        .map(|v| (v, BrightnessSource::Wmi))
        .or_else(|| ddc_brightness(near).map(|v| (v, BrightnessSource::Ddc)))
}

pub(crate) fn set_brightness(source: BrightnessSource, near: HWND, level: f32) -> bool {
    match source {
        BrightnessSource::Wmi => wmi_set_brightness(level),
        BrightnessSource::Ddc => ddc_set_brightness(near, level),
    }
}

pub(crate) fn step_brightness(level: f32, steps: i32) -> f32 {
    let pct = (level * 100.0).round() as i32 + steps * 5;
    pct.clamp(0, 100) as f32 / 100.0
}

fn enable_shutdown_privilege() {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .is_err()
        {
            return;
        }
        let mut luid = LUID::default();
        if LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid).is_ok() {
            let mut tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                ..Default::default()
            };
            tp.Privileges[0].Luid = luid;
            tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
            let _ = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
        }
        let _ = CloseHandle(token);
    }
}

pub(crate) fn sleep() {
    enable_shutdown_privilege();
    unsafe {
        let _ = SetSuspendState(false, false, false);
    }
}

pub(crate) fn restart() {
    enable_shutdown_privilege();
    unsafe {
        let _ = ExitWindowsEx(
            EWX_REBOOT,
            SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED,
        );
    }
}

pub(crate) fn shut_down() {
    enable_shutdown_privilege();
    unsafe {
        let _ = ExitWindowsEx(
            EWX_POWEROFF,
            SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED,
        );
    }
}

pub(crate) fn sign_out() {
    unsafe {
        let _ = WTSLogoffSession(
            WTS_CURRENT_SERVER_HANDLE,
            WTSGetActiveConsoleSessionId(),
            false,
        );
    }
}

pub(crate) fn lock() {
    unsafe {
        let _ = LockWorkStation();
    }
}

pub(crate) fn screenshot() {
    std::thread::sleep(std::time::Duration::from_millis(200));
    crate::pc_cursor::copy_screen_to_clipboard(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_steps_are_two_percent_and_clamped() {
        assert_eq!(step_volume(0.62, 1), 0.64);
        assert_eq!(step_volume(0.62, -1), 0.60);
        assert_eq!(step_volume(0.99, 1), 1.0);
        assert_eq!(step_volume(0.01, -1), 0.0);
        assert_eq!(step_volume(0.0, -5), 0.0);
        assert_eq!(step_volume(1.0, 5), 1.0);
        assert_eq!(step_brightness(0.8, 1), 0.85);
        assert_eq!(step_brightness(0.02, -1), 0.0);
    }

    #[test]
    fn app_and_output_names_are_short() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(
            app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "ZuneMusic"
        );
        assert_eq!(app_name("C:\\Program Files\\VLC\\vlc.exe"), "Vlc");
        assert_eq!(app_name(""), "");
        assert_eq!(short_output_name("Headphones (WH-1000XM4)"), "Headphones");
        assert_eq!(short_output_name("Speakers"), "Speakers");
    }
}
