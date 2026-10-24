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

fn marker_path() -> PathBuf {
    crate::paths::state_dir().join(".welcome_shown")
}
