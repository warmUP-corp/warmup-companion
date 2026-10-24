use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn write_report() -> Result<PathBuf, String> {
    let dir = crate::paths::state_dir().join("diagnostics");
    std::fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("warmup-companion-{stamp}.tar"));
    let files = vec![
        ("unit-status.txt".to_string(), capture(&["systemctl", "--user", "status", "warmup-companion", "--no-pager"])),
        ("journal.txt".to_string(), capture(&["journalctl", "--user", "-u", "warmup-companion", "-n", "200", "--no-pager"])),
        ("uinput-acl.txt".to_string(), uinput_acl()),
        ("sdl-devices.txt".to_string(), sdl_devices()),
        ("service-log.txt".to_string(), log_tail()),
        ("session.txt".to_string(), session_notes()),
        ("hyprctl-version.txt".to_string(), hyprctl("version")),
        ("hyprctl-devices.txt".to_string(), hyprctl("devices")),
        ("hyprctl-monitors.txt".to_string(), hyprctl("monitors")),
        ("library-watch.json".to_string(), state_file(&crate::paths::library_watch_file())),
        ("pending-play-sessions.json".to_string(), state_file(&crate::paths::pending_play_sessions_file())),
        ("parental-guard.json".to_string(), state_file(&crate::paths::parental_guard_file())),
        ("crash-report.txt".to_string(), state_file(&crate::crash::report_file())),
        ("crash-report.last.txt".to_string(), state_file(&crate::crash::last_report_file())),
    ];
    let bytes = ustar(&files);
    std::fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(path)
}

fn capture(argv: &[&str]) -> Vec<u8> {
    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..]);
    match cmd.output() {
        Ok(output) => {
            let mut body = output.stdout;
            if !output.stderr.is_empty() {
                body.extend_from_slice(b"\n");
                body.extend_from_slice(&output.stderr);
            }
            if body.is_empty() {
                format!("(no output, status {})\n", output.status).into_bytes()
            } else {
                body
            }
        }
        Err(error) => format!("failed to run {}: {error}\n", argv[0]).into_bytes(),
    }
}

fn hyprctl(arg: &str) -> Vec<u8> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return b"Hyprland is not the current compositor.\n".to_vec();
    }
    capture(&["hyprctl", arg])
}

fn uinput_acl() -> Vec<u8> {
    let mut body = capture(&["ls", "-l", "/dev/uinput"]);
    body.extend_from_slice(b"\n");
    body.extend(capture(&["getfacl", "-p", "/dev/uinput"]));
    body
}

fn sdl_devices() -> Vec<u8> {
    let mut body = format!(
        "companion connection: {}\n\n",
        crate::pipe_server::connection_name()
    );
    body.push_str("js devices:\n");
    match std::fs::read_dir("/dev/input") {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with("js") || name.starts_with("event"))
                .collect();
            names.sort();
            for name in names {
                body.push_str(&name);
                body.push('\n');
            }
        }
        Err(error) => body.push_str(&format!("read /dev/input: {error}\n")),
    }
    body.push_str("\nby-id:\n");
    match std::fs::read_dir("/dev/input/by-id") {
        Ok(entries) => {
            for entry in entries.filter_map(|entry| entry.ok()) {
                body.push_str(&entry.file_name().to_string_lossy());
                body.push('\n');
            }
        }
        Err(error) => body.push_str(&format!("read by-id: {error}\n")),
    }
    body.into_bytes()
}

fn log_tail() -> Vec<u8> {
    let path = crate::paths::log_file();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(240);
            let mut body = format!("log: {}\n", path.display());
            for line in &lines[start..] {
                body.push_str(line);
                body.push('\n');
            }
            body.into_bytes()
        }
        Err(error) => format!("read {}: {error}\n", path.display()).into_bytes(),
    }
}

fn state_file(path: &std::path::Path) -> Vec<u8> {
    match std::fs::read(path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            format!("{} does not exist.\n", path.display()).into_bytes()
        }
        Err(error) => format!("read {}: {error}\n", path.display()).into_bytes(),
    }
}

fn session_notes() -> Vec<u8> {
    let wayland = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let hypr = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    format!(
        "WAYLAND_DISPLAY={wayland}\nXDG_RUNTIME_DIR={runtime}\nHYPRLAND_INSTANCE_SIGNATURE={hypr}\nXDG_CURRENT_DESKTOP={desktop}\nsocket={}\n",
        crate::paths::socket_file().display()
    )
    .into_bytes()
}

fn ustar(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in files {
        out.extend(header(name, data.len() as u64));
        out.extend(data);
        let pad = (512 - (data.len() % 512)) % 512;
        out.extend(std::iter::repeat(0u8).take(pad));
    }
    out.extend(std::iter::repeat(0u8).take(1024));
    out
}

fn header(name: &str, size: u64) -> [u8; 512] {
    let mut raw = [0u8; 512];
    write_bytes(&mut raw[0..100], name.as_bytes());
    write_bytes(&mut raw[100..108], b"0000644");
    write_bytes(&mut raw[108..116], b"0000000");
    write_bytes(&mut raw[116..124], b"0000000");
    write_octal(&mut raw[124..136], size);
    write_octal(&mut raw[136..148], 0);
    for byte in &mut raw[148..156] {
        *byte = b' ';
    }
    raw[156] = b'0';
    write_bytes(&mut raw[257..263], b"ustar");
    write_bytes(&mut raw[263..265], b"00");
    let sum: u64 = raw.iter().map(|byte| *byte as u64).sum();
    write_octal(&mut raw[148..156], sum);
    raw[155] = b' ';
    raw
}

fn write_bytes(slot: &mut [u8], bytes: &[u8]) {
    let n = bytes.len().min(slot.len());
    slot[..n].copy_from_slice(&bytes[..n]);
}

fn write_octal(slot: &mut [u8], value: u64) {
    let text = format!("{value:0width$o}", width = slot.len().saturating_sub(1));
    let bytes = text.as_bytes();
    let n = bytes.len().min(slot.len());
    slot[..n].copy_from_slice(&bytes[..n]);
}

#[cfg(test)]
mod tests {
    use super::ustar;

    #[test]
    fn ustar_contains_the_file_name_and_body() {
        let bytes = ustar(&[("unit-status.txt".into(), b"active\n".to_vec())]);
        let header = String::from_utf8_lossy(&bytes[..100]);
        assert!(header.starts_with("unit-status.txt"));
        assert!(bytes.windows(7).any(|window| window == b"active\n"));
    }
}
