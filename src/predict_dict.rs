//! Personal dictionary load/save for VK prefix prediction.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

fn personal_dict_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(base)
            .join("WarmupKeyboard")
            .join("personal.dict"),
    )
}

pub fn load_personal(into: &mut HashSet<String>) {
    into.clear();
    let Some(path) = personal_dict_path() else {
        return;
    };
    let Ok(data) = fs::read_to_string(&path) else {
        return;
    };
    for line in data.lines() {
        let w = line.trim().to_ascii_lowercase();
        if w.len() >= 2 && w.chars().all(|c| c.is_ascii_alphabetic()) {
            into.insert(w);
        }
    }
}

pub fn flush_personal(from: &HashSet<String>) {
    let Some(path) = personal_dict_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut lines: Vec<&String> = from.iter().collect();
    lines.sort();
    let body = lines
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = fs::write(&path, body);
}
