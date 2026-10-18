use windows::core::{h, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

pub fn show_screenshot_toast(path: &str, copied: bool) {
    register_aumid();

    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let body = if copied {
        format!("{name} \u{b7} copied to clipboard")
    } else {
        name
    };
    let launch = file_uri(path);

    let xml = format!(
        r#"<toast activationType="protocol" launch="{launch}"><visual><binding template="ToastGeneric"><text>{title}</text><text>{body}</text></binding></visual></toast>"#,
        launch = xml_escape(&launch),
        title = xml_escape("Screenshot taken"),
        body = xml_escape(&body),
    );

    if let Err(e) = show(&xml) {
        crate::install::log_line(&format!("screenshot toast: {e}"));
    }

    std::thread::sleep(std::time::Duration::from_secs(1));
}

pub fn show_update_toast(version: &str) {
    register_aumid();
    register_update_protocol();
    let xml = format!(
        r#"<toast activationType="protocol" launch="warmup-companion:update?open" duration="long"><visual><binding template="ToastGeneric"><text>{title}</text><text>{body}</text></binding></visual><actions><action content="Install" activationType="protocol" arguments="warmup-companion:update?install"/><action content="Later" activationType="system" arguments="dismiss"/></actions></toast>"#,
        title = xml_escape(&format!("Warmup Companion v{version} is available")),
        body = xml_escape("Install it now, or later from the tray under Updates."),
    );
    if let Err(e) = show(&xml) {
        crate::install::log_line(&format!("update toast: {e}"));
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
}

fn register_update_protocol() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let command = format!("\"{}\" --update-url \"%1\"", exe.display());
    for (key, name, data) in [
        (r"Software\Classes\warmup-companion", None, "URL:warmUP Companion"),
        (r"Software\Classes\warmup-companion", Some("URL Protocol"), ""),
        (
            r"Software\Classes\warmup-companion\shell\open\command",
            None,
            command.as_str(),
        ),
    ] {
        set_hkcu_string(key, name, data);
    }
}

fn show(xml: &str) -> Result<(), String> {
    let doc = XmlDocument::new().map_err(|e| format!("XmlDocument::new: {e}"))?;
    doc.LoadXml(&HSTRING::from(xml))
        .map_err(|e| format!("LoadXml: {e}"))?;
    let toast = ToastNotification::CreateToastNotification(&doc)
        .map_err(|e| format!("CreateToastNotification: {e}"))?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(h!("warmUP.CompanionApp"))
        .map_err(|e| format!("CreateToastNotifierWithId: {e}"))?;
    notifier.Show(&toast).map_err(|e| format!("Show: {e}"))?;
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn file_uri(path: &str) -> String {
    let forward = path.replace('\\', "/");
    let encoded = forward.replace(' ', "%20");
    format!("file:///{encoded}")
}

fn register_aumid() {
    set_hkcu_string(
        r"Software\Classes\AppUserModelId\warmUP.CompanionApp",
        Some("DisplayName"),
        "warmUP",
    );
    if let Some(icon) = write_icon_png() {
        set_hkcu_string(
            r"Software\Classes\AppUserModelId\warmUP.CompanionApp",
            Some("IconUri"),
            &icon.display().to_string(),
        );
    }
}

fn write_icon_png() -> Option<std::path::PathBuf> {
    const ICO: &[u8] = include_bytes!("../../assets/icon.ico");
    let u16_at = |o: usize| u16::from_le_bytes([ICO[o], ICO[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(ICO[o..o + 4].try_into().unwrap()) as usize;
    let (size, offset) = (0..u16_at(4))
        .map(|i| 6 + 16 * i)
        .map(|e| (u32_at(e + 8), u32_at(e + 12)))
        .max_by_key(|(size, _)| *size)?;
    let png = ICO.get(offset..offset + size)?;
    let dir = std::path::Path::new(&std::env::var_os("LOCALAPPDATA")?).join("WarmupVk");
    let path = dir.join("toast-icon.png");
    if std::fs::read(&path).ok().as_deref() != Some(png) {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, png).ok()?;
    }
    Some(path)
}

fn set_hkcu_string(subkey: &str, name: Option<&str>, data: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let subkey_w = wide(subkey);
    let name_w = name.map(wide);
    let data_w = wide(data);

    unsafe {
        let mut hkey = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
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
            crate::install::log_line(&format!("toast: open {subkey} failed rc={}", rc.0));
            return;
        }
        let bytes = std::slice::from_raw_parts(data_w.as_ptr().cast::<u8>(), data_w.len() * 2);
        let value = name_w.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
        let rc = RegSetValueExW(hkey, value, 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
        if rc.0 != 0 {
            crate::install::log_line(&format!("toast: set {subkey} failed rc={}", rc.0));
        }
    }
}
