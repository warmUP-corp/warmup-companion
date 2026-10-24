use std::path::PathBuf;

pub fn show_welcome_if_first_run() {
    let marker = marker_path();
    if marker.exists() {
        return;
    }
    let body = "Connect a controller. The sticks move the pointer, the right stick scrolls, A clicks, and View or Start presses Enter.\n\nRight-click the tray icon for Controller Center, game detection, and diagnostics.";
    if let Err(error) = notify("Warmup Companion is ready", body) {
        crate::install::log_line(&format!("welcome notification failed: {error}"));
    }
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(marker, b"1");
}

pub fn notify(summary: &str, body: &str) -> Result<(), String> {
    notify_rust::Notification::new()
        .summary(summary)
        .body(body)
        .show()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn notify_with_actions(
    summary: &str,
    body: &str,
    actions: &[(&str, &str)],
) -> Result<String, String> {
    let mut notification = notify_rust::Notification::new();
    notification.summary(summary).body(body);
    for (id, label) in actions {
        notification.action(id, label);
    }
    let handle = notification.show().map_err(|error| error.to_string())?;
    let mut chosen = String::new();
    handle.wait_for_action(|action| chosen = action.to_string());
    Ok(chosen)
}

pub fn notify_screenshot(path: &std::path::Path, copied: bool) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let body = if copied {
        format!("{name} \u{b7} copied to clipboard")
    } else {
        name
    };
    let shown = notify_rust::Notification::new()
        .summary("Screenshot taken")
        .body(&body)
        .icon(&path.to_string_lossy())
        .action("default", "Open")
        .show();
    let handle = match shown {
        Ok(handle) => handle,
        Err(error) => {
            crate::install::log_line(&format!("screenshot notification failed: {error}"));
            return;
        }
    };
    let path = path.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("warmup-screenshot-toast".into())
        .spawn(move || {
            handle.wait_for_action(|action| {
                if action == "default" {
                    let _ = std::process::Command::new("xdg-open")
                        .arg(&path)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .map(|mut child| {
                            std::thread::spawn(move || child.wait());
                        });
                }
            });
        });
}

fn marker_path() -> PathBuf {
    crate::paths::state_dir().join(".welcome_shown")
}
