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

fn show(xml: &str) -> Result<(), String> {
    let doc = XmlDocument::new().map_err(|e| format!("XmlDocument::new: {e}"))?;
    doc.LoadXml(&HSTRING::from(xml))
        .map_err(|e| format!("LoadXml: {e}"))?;
    let toast = ToastNotification::CreateToastNotification(&doc)
        .map_err(|e| format!("CreateToastNotification: {e}"))?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(h!("warmUP.Companion"))
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
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let subkey_w = wide(r"Software\Classes\AppUserModelId\warmUP.Companion");
    let value_w = wide("DisplayName");
    let data_w = wide("warmUP");

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
            crate::install::log_line(&format!("screenshot toast: AUMID key open failed rc={}", rc.0));
            return;
        }
        let bytes =
            std::slice::from_raw_parts(data_w.as_ptr().cast::<u8>(), data_w.len() * 2);
        let rc = RegSetValueExW(hkey, PCWSTR(value_w.as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
        if rc.0 != 0 {
            crate::install::log_line(&format!(
                "screenshot toast: AUMID DisplayName set failed rc={}",
                rc.0
            ));
        }
    }
}
