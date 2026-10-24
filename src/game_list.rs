#![cfg(any(windows, target_os = "linux"))]

pub(crate) const BUILTIN_DENYLIST_EXES: &[&str] = &[
    "warmup-companion.exe",
    "warmup.exe",
    "applicationframehost.exe",
    "arc.exe",
    "brave.exe",
    "chatgpt.exe",
    "chrome.exe",
    "chromium.exe",
    "code.exe",
    "codex.exe",
    "comet.exe",
    "ctfmon.exe",
    "cursor.exe",
    "duckduckgo.exe",
    "dllhost.exe",
    "dwm.exe",
    "explorer.exe",
    "firefox.exe",
    "lockapp.exe",
    "msedge.exe",
    "msedgewebview2.exe",
    "opera.exe",
    "perplexity.exe",
    "runtimebroker.exe",
    "searchhost.exe",
    "searchapp.exe",
    "shellhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "steamwebhelper.exe",
    "systemsettings.exe",
    "textinputhost.exe",
    "webviewhost.exe",
    "widgets.exe",
    "windowsterminal.exe",
    "wt.exe",
    "steam.exe",
    "epicgameslauncher.exe",
    "galaxyclient.exe",
    "gog galaxy.exe",
    "goggalaxy.exe",
    "eadesktop.exe",
    "origin.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "uplay.exe",
    "battle.net.exe",
    "amazon games.exe",
    "nile.exe",
    "xboxpcapp.exe",
    "gamingservices.exe",
    "launcherpatcher.exe",
    "rockstarservice.exe",
    "rockstarsteamhelper.exe",
    "rockstarerrorhandler.exe",
    "socialclubhelper.exe",
];

pub(crate) const AMBIGUOUS_STORE_CLIENT_EXES: &[&str] = &["launcher.exe"];

const STORE_CLIENT_PATH_HINTS: &[&str] = &["/rockstar games/launcher/"];

pub(crate) const BUILTIN_BACKGROUND_TOOL_STEMS: &[&str] = &["wallpaper32", "wallpaper64"];

pub(crate) const BUILTIN_BACKGROUND_TOOL_STEAM_APP_IDS: &[&str] = &["431960"];

pub(crate) fn normalize(path: &str) -> String {
    path.trim().replace('\\', "/").to_ascii_lowercase()
}

pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub(crate) fn is_store_client_path(image_path: &str) -> bool {
    let path = normalize(image_path);
    STORE_CLIENT_PATH_HINTS
        .iter()
        .any(|hint| path.contains(hint))
}

pub(crate) fn is_denylisted_exe(exe_file_name: &str, image_path: Option<&str>) -> bool {
    let base = normalize(file_name(&normalize(exe_file_name)));
    if base.is_empty() {
        return false;
    }
    if BUILTIN_DENYLIST_EXES.contains(&base.as_str()) {
        return true;
    }
    if AMBIGUOUS_STORE_CLIENT_EXES.contains(&base.as_str()) {
        return image_path.is_some_and(is_store_client_path);
    }
    let stem = base.strip_suffix(".exe").unwrap_or(&base);
    BUILTIN_BACKGROUND_TOOL_STEMS.contains(&stem)
}
