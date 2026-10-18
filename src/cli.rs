//! Early-process CLI dispatch (install, service worker, settings, help text).

#[cfg(windows)]
pub fn dispatch_install_or_service(args: &[String]) {
    if let Some(i) = args.iter().position(|a| a == "--toast-helper") {
        let path = args.get(i + 1).map(String::as_str).unwrap_or_default();
        let copied = args.iter().any(|a| a == "--copied");
        crate::win::toast::show_screenshot_toast(path, copied);
        std::process::exit(0);
    }
    if let Some(i) = args.iter().position(|a| a == "--update-toast-helper") {
        let version = args.get(i + 1).map(String::as_str).unwrap_or_default();
        crate::win::toast::show_update_toast(version);
        std::process::exit(0);
    }
    #[cfg(feature = "gamepad")]
    if let Some(i) = args.iter().position(|a| a == "--update-url") {
        crate::updater::signal_from_url(args.get(i + 1).map(String::as_str).unwrap_or_default());
        std::process::exit(0);
    }
    // Mic recognition runs here, as the real logged-in user (the worker spawns us
    // via CreateProcessAsUserW). Short-lived: recognize until silence, then exit.
    if args.iter().any(|a| a == "--speech-helper") {
        let code = match crate::win::speech_input::run_blocking() {
            Ok(()) => 0,
            Err(e) => {
                crate::install::log_line(&format!("speech helper failed: {e}"));
                1
            }
        };
        std::process::exit(code);
    }
    // Resident parakeet model host: loaded once, kept warm across mic toggles. Spawned
    // detached by the speech helper (already in the user session) on first parakeet use.
    if args.iter().any(|a| a == "--parakeet-server") {
        let code = match crate::win::speech_input::run_parakeet_server() {
            Ok(()) => 0,
            Err(e) => {
                crate::install::log_line(&format!("parakeet-server failed: {e}"));
                1
            }
        };
        std::process::exit(code);
    }
    match args.get(1).map(String::as_str) {
        Some("setup") => {
            #[cfg(feature = "gamepad")]
            {
                let code = crate::win::installer_ui::run(args);
                std::process::exit(code);
            }
            #[cfg(not(feature = "gamepad"))]
            {
                eprintln!("setup UI requires the gamepad feature");
                std::process::exit(1);
            }
        }
        Some("uninstall-ui") => {
            #[cfg(feature = "gamepad")]
            {
                let code = crate::win::installer_ui::run(args);
                std::process::exit(code);
            }
            #[cfg(not(feature = "gamepad"))]
            {
                eprintln!("uninstall UI requires the gamepad feature");
                std::process::exit(3);
            }
        }
        Some("install") => {
            let debug_ui = args.iter().any(|a| a == "--debug-ui" || a == "--debug");
            crate::install::run_install(debug_ui);
            std::process::exit(0);
        }
        Some("install-dev") => {
            let Some(path) = args.get(2) else {
                eprintln!("usage: warmup-companion.exe install-dev <path-to-warmup.exe>");
                std::process::exit(2);
            };
            crate::install::run_install_dev(std::path::Path::new(path));
            std::process::exit(0);
        }
        Some("uninstall") => {
            crate::install::run_uninstall();
            std::process::exit(0);
        }
        Some("verify") => {
            crate::install::run_verify();
            std::process::exit(0);
        }
        Some("stop") => {
            crate::install::run_stop();
            std::process::exit(0);
        }
        Some("grant-service-acl") => {
            crate::install::run_grant_interactive_control();
            std::process::exit(0);
        }
        Some("restore-keyboard") | Some("restore-native-keyboard") => {
            crate::win::native_keyboard::restore_auto_invoke();
            crate::win::native_keyboard::ensure_search_service_running();
            crate::install::log_line(
                "restore-keyboard: requested Windows keyboard service restore",
            );
            println!("Requested Windows touch keyboard/search service restore.");
            std::process::exit(0);
        }
        #[cfg(feature = "service")]
        Some("wake") => match crate::service::wake() {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
        #[cfg(feature = "gamepad")]
        Some("settings") => {
            run_settings_command(args);
            std::process::exit(0);
        }
        _ => {}
    }
    #[cfg(feature = "service")]
    {
        if args.iter().any(|a| a == "--service-worker") {
            #[cfg(feature = "gamepad")]
            crate::tray::spawn();
            match crate::service::run_worker() {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    crate::install::log_line(&format!("service worker failed: {e}"));
                    std::process::exit(1);
                }
            }
        }
        let scm_start = args.len() <= 1 && !has_interactive_console();
        let force_service = args.iter().any(|a| a == "--service");
        if scm_start || force_service {
            if crate::service::run_dispatcher().is_ok() {
                std::process::exit(0);
            } else if force_service {
                crate::install::log_line("--service: not running under SCM");
                std::process::exit(1);
            }
        }
    }
    #[cfg(not(feature = "service"))]
    if args.iter().any(|a| a == "--service") {
        eprintln!("Rebuild with default features enabled: cargo build --release");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn has_interactive_console() -> bool {
    use windows::Win32::System::Console::GetConsoleWindow;
    unsafe {
        let hwnd = GetConsoleWindow();
        !hwnd.0.is_null()
    }
}

#[cfg(feature = "gamepad")]
fn run_settings_command(args: &[String]) {
    #[cfg(windows)]
    let usage = "usage:
  warmup-companion.exe settings get
  warmup-companion.exe settings path
  warmup-companion.exe settings set <key> <value>
  warmup-companion.exe settings sleep-on-game <get|on|off>
  warmup-companion.exe settings auto-stop-on-game <get|on|off>
  warmup-companion.exe settings userland-poll <get|full|sleep|path>";
    #[cfg(not(windows))]
    let usage = "usage:
  warmup-companion settings get
  warmup-companion settings path
  warmup-companion settings set <key> <value>
  warmup-companion settings sleep-on-game <get|on|off>
  warmup-companion settings auto-stop-on-game <get|on|off>
  warmup-companion settings userland-poll <get|full|sleep|path>";
    match args.get(2).map(String::as_str) {
        Some("get") | None => print_gamepad_settings(),
        Some("path") => match crate::config::settings_path() {
            Some(path) => println!("{}", path.display()),
            None => {
                eprintln!("settings path is unavailable");
                std::process::exit(1);
            }
        },
        Some("set") => {
            let Some(key) = args.get(3) else {
                eprintln!("{usage}");
                std::process::exit(2);
            };
            let Some(value) = args.get(4) else {
                eprintln!("{usage}");
                std::process::exit(2);
            };
            if let Err(e) = crate::config::set_gamepad_setting(key, value) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            println!("{key}={value}");
        }
        Some("userland-poll") => match args.get(3).map(String::as_str) {
            Some("get") | None => {
                let mode = crate::config::userland_gamepad_poll_mode();
                println!("{}", poll_mode_name(mode));
            }
            Some("full") => {
                if let Err(e) =
                    crate::config::set_userland_gamepad_poll_mode(warmup_gamepad::PollMode::Full)
                {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("full");
            }
            Some("sleep") | Some("guide") | Some("guide-only") => {
                if let Err(e) =
                    crate::config::set_userland_gamepad_poll_mode(warmup_gamepad::PollMode::Sleep)
                {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("sleep");
            }
            Some("path") => match crate::config::settings_path() {
                Some(path) => println!("{}", path.display()),
                None => {
                    eprintln!("settings path is unavailable");
                    std::process::exit(1);
                }
            },
            Some(_) => {
                eprintln!("{usage}");
                std::process::exit(2);
            }
        },
        Some("sleep-on-game") => match args.get(3).map(String::as_str) {
            Some("get") | None => {
                println!("{}", crate::config::gamepad_settings().sleep_on_game);
            }
            Some("on") | Some("true") | Some("1") => {
                if let Err(e) = crate::config::set_gamepad_setting("sleep_on_game", "true") {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("true");
            }
            Some("off") | Some("false") | Some("0") => {
                if let Err(e) = crate::config::set_gamepad_setting("sleep_on_game", "false") {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("false");
            }
            Some(_) => {
                eprintln!("{usage}");
                std::process::exit(2);
            }
        },
        Some("auto-stop-on-game") => match args.get(3).map(String::as_str) {
            Some("get") | None => {
                println!("{}", crate::config::gamepad_settings().auto_stop_on_game);
            }
            Some("on") | Some("true") | Some("1") => {
                if let Err(e) = crate::config::set_gamepad_setting("auto_stop_on_game", "true") {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("true");
            }
            Some("off") | Some("false") | Some("0") => {
                if let Err(e) = crate::config::set_gamepad_setting("auto_stop_on_game", "false") {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("false");
            }
            Some(_) => {
                eprintln!("{usage}");
                std::process::exit(2);
            }
        },
        Some(_) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    }
}

#[cfg(feature = "gamepad")]
fn poll_mode_name(mode: warmup_gamepad::PollMode) -> &'static str {
    match mode {
        warmup_gamepad::PollMode::Full => "full",
        warmup_gamepad::PollMode::Sleep => "sleep",
    }
}

#[cfg(feature = "gamepad")]
fn print_gamepad_settings() {
    let s = crate::config::gamepad_settings();
    println!("userland_poll={}", poll_mode_name(s.userland_poll_mode));
    println!("sleep_on_game={}", s.sleep_on_game);
    println!("auto_stop_on_game={}", s.auto_stop_on_game);
    println!("signin_hints={}", s.signin_hints);
    println!("voice_enabled={}", s.voice_enabled);
    println!("cursor_deadzone={}", s.cursor_deadzone);
    println!("cursor_speed={}", s.cursor_speed);
    println!("cursor_accel={}", s.cursor_accel);
    println!("scroll_deadzone={}", s.scroll_deadzone);
    println!("scroll_speed={}", s.scroll_speed);
    println!("scroll_accel={}", s.scroll_accel);
}

#[cfg(all(target_os = "linux", feature = "gamepad"))]
fn run_update_command(args: &[String]) -> i32 {
    use crate::linux_update::{Check, Options, Outcome, Restart};
    let flag = |name: &str| args.iter().any(|a| a == name);
    if flag("--check") {
        return match crate::linux_update::check() {
            Ok(Check::UpToDate(version)) => {
                println!(
                    "Up to date: v{} (latest release v{version}).",
                    crate::release_feed::CURRENT
                );
                0
            }
            Ok(Check::NoLinuxBuild(version)) => {
                println!(
                    "v{version} is out, but it has no Linux build ({}).",
                    crate::linux_update::asset_name()
                );
                0
            }
            Ok(Check::Available(release)) => {
                println!(
                    "v{} is available ({} bytes).",
                    release.version, release.size
                );
                for note in &release.notes {
                    println!("  - {note}");
                }
                println!("{}", release.page);
                0
            }
            Err(error) => {
                eprintln!("update check failed: {error}");
                1
            }
        };
    }
    let mut options = Options::installed();
    options.restart = Restart::Wait;
    options.dry_run = flag("--dry-run");
    if let Some(i) = args.iter().position(|a| a == "--dir") {
        let Some(dir) = args.get(i + 1) else {
            eprintln!("usage: warmup-companion update [--check] [--dry-run] [--dir <path>]");
            return 2;
        };
        options.dir = std::path::PathBuf::from(dir);
        options.restart = Restart::Skip;
    }
    match crate::linux_update::update(&options) {
        Ok(Outcome::UpToDate(version)) => {
            println!(
                "Up to date: v{} (latest release v{version}).",
                crate::release_feed::CURRENT
            );
            0
        }
        Ok(Outcome::NoLinuxBuild(version)) => {
            println!("v{version} is out, but it has no Linux build. Nothing was changed.");
            0
        }
        Ok(Outcome::Verified(release)) => {
            println!(
                "Dry run: v{} downloaded and its checksum matches. Nothing was replaced.",
                release.version
            );
            0
        }
        Ok(Outcome::Installed { release, restarted }) => {
            println!(
                "Installed v{} in {}.",
                release.version,
                options.dir.display()
            );
            if restarted {
                println!("Restarted warmup-companion.service.");
            } else {
                println!("The service was not restarted.");
            }
            0
        }
        Err(error) => {
            eprintln!("update failed: {error}");
            1
        }
    }
}

#[cfg(target_os = "linux")]
pub fn dispatch_install_or_service(args: &[String]) {
    #[cfg(feature = "speech")]
    if args.iter().any(|arg| arg == "--parakeet-server") {
        let code = match crate::linux_speech::run_server() {
            Ok(()) => 0,
            Err(error) => {
                crate::install::log_line(&format!("parakeet-server failed: {error}"));
                1
            }
        };
        std::process::exit(code);
    }
    match args.get(1).map(String::as_str) {
        Some("install") => {
            crate::install::run_install();
            std::process::exit(0);
        }
        Some("uninstall") => {
            crate::install::run_uninstall();
            std::process::exit(0);
        }
        Some("verify") => {
            crate::install::run_verify();
            std::process::exit(0);
        }
        Some("stop") => {
            crate::install::run_stop();
            std::process::exit(0);
        }
        #[cfg(feature = "gamepad")]
        Some("keyboard") => {
            match crate::linux_vk::Overlay::open(false) {
                Ok(overlay) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let _ = ctrlc::set_handler(move || {
                        let _ = tx.send(());
                    });
                    let _ = rx.recv_timeout(std::time::Duration::from_secs(20));
                    overlay.close();
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
        Some("type") => {
            let text = args.iter().skip(2).cloned().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                eprintln!("usage: warmup-companion type <text>");
                std::process::exit(2);
            }
            crate::vk_nav::send_text_direct(&text);
            std::process::exit(0);
        }
        #[cfg(feature = "gamepad")]
        Some("diagnostics") => {
            match crate::linux_diag::write_report() {
                Ok(path) => {
                    println!("{}", path.display());
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("diagnostics failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        #[cfg(feature = "gamepad")]
        Some("settings") => {
            run_settings_command(args);
            std::process::exit(0);
        }
        #[cfg(feature = "gamepad")]
        Some("update") => {
            std::process::exit(run_update_command(args));
        }
        _ => {}
    }
    if args.iter().any(|a| a == "--controller-center") {
        #[cfg(feature = "gamepad")]
        {
            if let Err(error) = crate::linux_center::show_blocking() {
                eprintln!("{error}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        #[cfg(not(feature = "gamepad"))]
        {
            eprintln!("Rebuild with default features enabled: cargo build --release");
            std::process::exit(1);
        }
    }
    if args.iter().any(|a| a == "--service") {
        #[cfg(feature = "service")]
        {
            match crate::linux_service::run() {
                Ok(()) => std::process::exit(0),
                Err(error) => {
                    crate::install::log_line(&format!("service failed: {error}"));
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
        #[cfg(not(feature = "service"))]
        {
            eprintln!("Rebuild with default features enabled: cargo build --release");
            std::process::exit(1);
        }
    }
}

pub fn help_screen_rows(help: &str) -> u32 {
    let cols = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(120)
        .max(40);
    let mut rows = 0u32;
    for line in help.split('\n') {
        let w = line.chars().count() as u32;
        rows = rows.saturating_add(if w == 0 { 1 } else { w.div_ceil(cols) });
    }
    // println! adds one '\n' after HELP → cursor sits on following row
    rows.saturating_add(1)
}

pub fn print_help() {
    const HELP: &str = r#"COMMANDS
  normal              start normal user instance on default desktop
  cfg winlogon on     set config.bin +0xd9
  cfg winlogon off    clear config.bin +0xd9
  boot                start -boot service path
  fg normal           foreground normal user app
  fg uac              foreground UAC consent, input desktop winlogon
  fg logon            foreground LogonUI.exe, input desktop winlogon
  fg lock             foreground LockApp.exe, input desktop winlogon
  fg fullscreen       foreground fullscreen app, profile flag on
  attach input        warmup_attach_input_desktop
  press               mask 0x200 -> warmup_process_controller_input
  release             warmup_on_controller_release
  spiral on/off       g_app_feature_flags bit 9 -> Spiral vs Xbox path
  block on/off        toggle state[0x2c] bit 4
  mask on/off         toggle physical mask bit 0x200
  slot good           slot 7 type 6 subtype 7 -> queue action 7
  slot bad            slot 7 does not queue action 7
  reset               reset state
  quit                exit
  --real              Win32 desktop + TabTip/WarmupXboxVkWindow (Windows)
  pad                 (gamepad feature) SDL3 snapshot
  --gamepad           (gamepad feature) sticks + L3 → VK

SCENARIOS
  normal -> fg uac -> press
  cfg winlogon on -> boot -> fg logon -> press
  cfg winlogon on -> boot -> fg uac -> press -> press
"#;
    crate::repl_scroll::note_lines(help_screen_rows(HELP));
    println!("{HELP}");
}
