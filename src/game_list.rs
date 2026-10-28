#![cfg(any(windows, target_os = "linux"))]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Once, RwLock};

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

#[cfg(windows)]
pub(crate) fn is_denylisted_exe(exe_file_name: &str, image_path: Option<&str>) -> bool {
    let base = normalize(file_name(&normalize(exe_file_name)));
    if base.is_empty() {
        return false;
    }
    if AMBIGUOUS_STORE_CLIENT_EXES.contains(&base.as_str()) {
        return image_path.is_some_and(is_store_client_path);
    }
    let listed = match current() {
        Some(index) if !index.denylist_exes.is_empty() => index.denylist_exes.contains(&base),
        _ => BUILTIN_DENYLIST_EXES.contains(&base.as_str()),
    };
    let stem = base.strip_suffix(".exe").unwrap_or(&base);
    listed
        || is_background_tool_stem(stem)
        || blacklist()
            .iter()
            .any(|name| name.strip_suffix(".exe").unwrap_or(name) == stem)
}

pub(crate) fn blacklist() -> Vec<String> {
    crate::paths::settings_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse_blacklist(&text))
        .unwrap_or_default()
}

pub(crate) fn parse_blacklist(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| key.trim() == "game_blacklist")
        .flat_map(|(_, value)| value.split(','))
        .map(|name| normalize(file_name(&normalize(name))))
        .filter(|name| !name.is_empty())
        .collect()
}

const MAX_TOTAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_PATTERNS: usize = 20_000;
const MAX_STEAM_APP_IDS: usize = 30_000;
const MAX_LOCAL_GAME_DIRS: usize = 4_000;
const MAX_NAME_LIST: usize = 4_000;
const STEAM_LIBRARY_SEGMENT: &str = "/steamapps/common/";

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GameList {
    pub revision: u64,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub steam_app_ids: Vec<String>,
    #[serde(default)]
    pub local_game_dirs: Vec<crate::protocol::LocalGameDir>,
    #[serde(default)]
    pub denylist_exes: Vec<String>,
    #[serde(default)]
    pub background_tool_stems: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GameListError {
    TooManyPatterns,
    TooManySteamAppIds,
    TooManyLocalGameDirs,
    TooManyNames,
    TooLarge,
    BadChunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkOutcome {
    Pending,
    Installed(u64),
}

struct Assembly {
    chunks: u32,
    received: Vec<bool>,
    list: GameList,
}

const MAX_CHUNKS: u32 = 128;

static ASSEMBLY: Mutex<Option<Assembly>> = Mutex::new(None);

struct Index {
    revision: u64,
    patterns_by_file_name: HashMap<String, Vec<String>>,
    steam_app_ids: HashSet<String>,
    local_dirs: Vec<(String, String)>,
    denylist_exes: HashSet<String>,
    background_tool_stems: HashSet<String>,
}

static INDEX: RwLock<Option<Arc<Index>>> = RwLock::new(None);
static LOADED: Once = Once::new();

pub(crate) fn validate(list: &GameList) -> Result<(), GameListError> {
    if list.patterns.len() > MAX_PATTERNS {
        return Err(GameListError::TooManyPatterns);
    }
    if list.steam_app_ids.len() > MAX_STEAM_APP_IDS {
        return Err(GameListError::TooManySteamAppIds);
    }
    if list.local_game_dirs.len() > MAX_LOCAL_GAME_DIRS {
        return Err(GameListError::TooManyLocalGameDirs);
    }
    if list.denylist_exes.len() > MAX_NAME_LIST || list.background_tool_stems.len() > MAX_NAME_LIST
    {
        return Err(GameListError::TooManyNames);
    }
    let total: usize = list
        .patterns
        .iter()
        .chain(&list.steam_app_ids)
        .chain(&list.denylist_exes)
        .chain(&list.background_tool_stems)
        .map(String::len)
        .chain(
            list.local_game_dirs
                .iter()
                .map(|dir| dir.game_id.len() + dir.prefix.len()),
        )
        .sum();
    if total > MAX_TOTAL_BYTES {
        return Err(GameListError::TooLarge);
    }
    Ok(())
}

fn dir_prefix(prefix: &str) -> Option<String> {
    let mut dir = normalize(prefix.trim_matches('"'));
    while dir.ends_with('/') {
        dir.pop();
    }
    if dir.is_empty() {
        return None;
    }
    dir.push('/');
    Some(dir)
}

fn build(list: &GameList) -> Index {
    let mut patterns_by_file_name: HashMap<String, Vec<String>> = HashMap::new();
    for pattern in list
        .patterns
        .iter()
        .map(|pattern| normalize(pattern.trim_start_matches('>')))
    {
        if pattern.is_empty() {
            continue;
        }
        patterns_by_file_name
            .entry(file_name(&pattern).to_string())
            .or_default()
            .push(pattern);
    }
    let local_dirs = list
        .local_game_dirs
        .iter()
        .filter(|dir| !dir.game_id.trim().is_empty())
        .filter_map(|dir| dir_prefix(&dir.prefix).map(|prefix| (prefix, dir.game_id.clone())))
        .collect();
    Index {
        revision: list.revision,
        patterns_by_file_name,
        steam_app_ids: list
            .steam_app_ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect(),
        local_dirs,
        denylist_exes: list
            .denylist_exes
            .iter()
            .map(|exe| normalize(exe))
            .filter(|exe| !exe.is_empty())
            .collect(),
        background_tool_stems: list
            .background_tool_stems
            .iter()
            .map(|stem| normalize(stem))
            .filter(|stem| !stem.is_empty())
            .collect(),
    }
}

fn current() -> Option<Arc<Index>> {
    LOADED.call_once(|| {
        if !cfg!(test) {
            load_persisted();
        }
    });
    INDEX.read().ok().and_then(|slot| slot.clone())
}

fn load_persisted() {
    let path = crate::paths::game_list_file();
    let Ok(meta) = std::fs::metadata(&path) else {
        return;
    };
    if meta.len() > MAX_TOTAL_BYTES as u64 * 2 {
        crate::install::log_line(&format!(
            "game list: ignored {} of {} bytes, over the size limit",
            path.display(),
            meta.len()
        ));
        return;
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return;
    };
    let list = match serde_json::from_str::<GameList>(&raw) {
        Ok(list) => list,
        Err(error) => {
            crate::install::log_line(&format!(
                "game list: ignored unreadable {}: {error}",
                path.display()
            ));
            return;
        }
    };
    if let Err(error) = validate(&list) {
        crate::install::log_line(&format!(
            "game list: ignored {} outside the limits: {error:?}",
            path.display()
        ));
        return;
    }
    let index = build(&list);
    crate::install::log_line(&format!(
        "game list: loaded revision {} ({} exe names, {} Steam apps, {} local folders)",
        index.revision,
        index.patterns_by_file_name.len(),
        index.steam_app_ids.len(),
        index.local_dirs.len()
    ));
    if let Ok(mut slot) = INDEX.write() {
        *slot = Some(Arc::new(index));
    }
}

pub(crate) fn install(list: &GameList) -> Result<(), GameListError> {
    validate(list)?;
    LOADED.call_once(|| {});
    let index = build(list);
    crate::install::log_line(&format!(
        "game list: installed revision {} ({} exe names, {} Steam apps, {} local folders)",
        index.revision,
        index.patterns_by_file_name.len(),
        index.steam_app_ids.len(),
        index.local_dirs.len()
    ));
    if let Ok(mut slot) = INDEX.write() {
        *slot = Some(Arc::new(index));
    }
    if let Ok(json) = serde_json::to_string(list) {
        let path = crate::paths::game_list_file();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let pending = path.with_extension("json.tmp");
        if std::fs::write(&pending, json).is_ok() && std::fs::rename(&pending, &path).is_err() {
            let _ = std::fs::remove_file(&pending);
        }
    }
    Ok(())
}

pub(crate) fn reset_assembly() {
    if let Ok(mut slot) = ASSEMBLY.lock() {
        *slot = None;
    }
}

pub(crate) fn has_index() -> bool {
    current().is_some_and(|index| {
        !index.patterns_by_file_name.is_empty()
            || !index.steam_app_ids.is_empty()
            || !index.local_dirs.is_empty()
    })
}

pub(crate) fn revision() -> Option<u64> {
    current().map(|index| index.revision)
}

pub(crate) fn is_detectable_image(image_path: &str) -> bool {
    let Some(index) = current() else {
        return false;
    };
    let path = normalize(image_path);
    index
        .patterns_by_file_name
        .get(file_name(&path))
        .is_some_and(|patterns| {
            patterns
                .iter()
                .any(|pattern| path.contains(pattern.as_str()))
        })
}

pub(crate) fn is_listed_steam_app(app_id: &str) -> bool {
    current().is_some_and(|index| index.steam_app_ids.contains(app_id.trim()))
}

pub(crate) fn is_steam_library_path(image_path: &str) -> bool {
    normalize(image_path).contains(STEAM_LIBRARY_SEGMENT)
}

pub(crate) fn local_game_dir_for(image_path: &str) -> Option<String> {
    let index = current()?;
    let path = normalize(image_path);
    let mut best: Option<(&str, &str)> = None;
    let mut tie = false;
    for (prefix, game_id) in &index.local_dirs {
        if !path.starts_with(prefix.as_str()) {
            continue;
        }
        match best {
            Some((best_prefix, best_game)) if prefix.len() == best_prefix.len() => {
                if best_game != game_id {
                    tie = true;
                }
            }
            Some((best_prefix, _)) if prefix.len() < best_prefix.len() => {}
            _ => {
                best = Some((prefix, game_id));
                tie = false;
            }
        }
    }
    if tie {
        return None;
    }
    best.map(|(_, game_id)| game_id.to_string())
}

pub(crate) fn is_background_tool_stem(stem: &str) -> bool {
    let stem = normalize(stem);
    match current() {
        Some(index) if !index.background_tool_stems.is_empty() => {
            index.background_tool_stems.contains(&stem)
        }
        _ => BUILTIN_BACKGROUND_TOOL_STEMS.contains(&stem.as_str()),
    }
}

pub(crate) fn is_background_tool_steam_app(app_id: &str) -> bool {
    BUILTIN_BACKGROUND_TOOL_STEAM_APP_IDS.contains(&app_id.trim())
}

#[cfg(target_os = "linux")]
pub(crate) fn denylist_names() -> Vec<String> {
    let (exes, stems) = match current() {
        Some(index) => (
            (!index.denylist_exes.is_empty())
                .then(|| index.denylist_exes.iter().cloned().collect()),
            (!index.background_tool_stems.is_empty())
                .then(|| index.background_tool_stems.iter().cloned().collect()),
        ),
        None => (None, None),
    };
    let exes: Vec<String> = exes.unwrap_or_else(|| {
        BUILTIN_DENYLIST_EXES
            .iter()
            .map(|exe| exe.to_string())
            .collect()
    });
    let stems: Vec<String> = stems.unwrap_or_else(|| {
        BUILTIN_BACKGROUND_TOOL_STEMS
            .iter()
            .map(|stem| stem.to_string())
            .collect()
    });
    exes.into_iter().chain(stems).chain(blacklist()).collect()
}

pub(crate) fn accept_chunk(
    payload: &crate::protocol::GameDetectionPayload,
) -> Result<ChunkOutcome, GameListError> {
    if payload.chunks == 0 || payload.chunks > MAX_CHUNKS || payload.chunk >= payload.chunks {
        return Err(GameListError::BadChunk);
    }
    let mut slot = ASSEMBLY.lock().map_err(|_| GameListError::BadChunk)?;
    let fresh = slot.as_ref().is_none_or(|assembly| {
        assembly.list.revision != payload.revision || assembly.chunks != payload.chunks
    });
    if fresh {
        *slot = Some(Assembly {
            chunks: payload.chunks,
            received: vec![false; payload.chunks as usize],
            list: GameList {
                revision: payload.revision,
                ..GameList::default()
            },
        });
    }
    let Some(assembly) = slot.as_mut() else {
        return Err(GameListError::BadChunk);
    };
    let index = payload.chunk as usize;
    if !assembly.received[index] {
        assembly.received[index] = true;
        let list = &mut assembly.list;
        list.patterns.extend(payload.patterns.iter().cloned());
        list.steam_app_ids
            .extend(payload.steam_app_ids.iter().cloned());
        list.local_game_dirs
            .extend(payload.local_game_dirs.iter().cloned());
        list.denylist_exes
            .extend(payload.denylist_exes.iter().cloned());
        list.background_tool_stems
            .extend(payload.background_tool_stems.iter().cloned());
        if let Err(error) = validate(list) {
            *slot = None;
            return Err(error);
        }
    }
    if !assembly.received.iter().all(|got| *got) {
        return Ok(ChunkOutcome::Pending);
    }
    let Some(done) = slot.take() else {
        return Err(GameListError::BadChunk);
    };
    drop(slot);
    install(&done.list)?;
    Ok(ChunkOutcome::Installed(done.list.revision))
}

const GENERIC_EXE_STEMS: &[&str] = &[
    "app",
    "bootstrap",
    "bootstrapper",
    "client",
    "crashpad_handler",
    "game",
    "gamelaunchhelper",
    "install",
    "launch",
    "launcher",
    "main",
    "play",
    "run",
    "setup",
    "start",
    "update",
    "updater",
];

pub(crate) fn is_generic_exe_stem(stem: &str) -> bool {
    GENERIC_EXE_STEMS.contains(&stem)
}

#[cfg(windows)]
pub(crate) fn exe_stem(exe_name: &str) -> String {
    let lower = exe_name.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .unwrap_or(lower.as_str())
        .to_string()
}

#[cfg(not(windows))]
pub(crate) fn exe_stem(exe_name: &str) -> String {
    let stem = exe_name.split('.').next().unwrap_or(exe_name);
    if stem.is_empty() {
        exe_name.to_ascii_lowercase()
    } else {
        stem.to_ascii_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessMatch {
    pub pid: u32,
    pub game_id: Option<String>,
    pub exe_stem: String,
    pub image_path: String,
    pub steam_app_id: Option<String>,
}

pub(crate) fn match_processes(
    processes: &[(u32, String, Option<String>)],
    games: &[crate::protocol::LibraryWatchGameEntry],
    steam_app_id: impl Fn(u32) -> Option<String>,
) -> Vec<ProcessMatch> {
    let indexed = has_index();
    let mut out = Vec::new();
    for (pid, exe_name, image_path) in processes {
        let Some(image_path) = image_path.as_deref().filter(|path| !path.trim().is_empty()) else {
            continue;
        };
        let stem = exe_stem(file_name(&normalize(exe_name)));
        if stem.is_empty() || is_background_tool_stem(&stem) {
            continue;
        }
        let steam_id = is_steam_library_path(image_path)
            .then(|| steam_app_id(*pid))
            .flatten();
        if steam_id
            .as_deref()
            .is_some_and(is_background_tool_steam_app)
        {
            continue;
        }
        let local = local_game_dir_for(image_path);
        let known = !indexed
            || is_detectable_image(image_path)
            || steam_id.as_deref().is_some_and(is_listed_steam_app)
            || local.is_some();
        if !known {
            continue;
        }
        let game_id = local.or_else(|| watch_owner(&stem, &normalize(image_path), games));
        if game_id.is_none() && !indexed {
            continue;
        }
        out.push(ProcessMatch {
            pid: *pid,
            game_id,
            exe_stem: stem,
            image_path: image_path.to_string(),
            steam_app_id: steam_id,
        });
    }
    out
}

pub(crate) fn watch_owner(
    stem: &str,
    normalized_image: &str,
    games: &[crate::protocol::LibraryWatchGameEntry],
) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    let mut tie = false;
    for game in games {
        for prefix in game
            .install_dir_prefixes
            .iter()
            .filter_map(|p| dir_prefix(p))
        {
            if !normalized_image.starts_with(prefix.as_str()) {
                continue;
            }
            match best {
                Some((len, id)) if prefix.len() == len => {
                    if id != game.game_id {
                        tie = true;
                    }
                }
                Some((len, _)) if prefix.len() < len => {}
                _ => {
                    best = Some((prefix.len(), game.game_id.as_str()));
                    tie = false;
                }
            }
        }
    }
    if let Some((_, id)) = best {
        return (!tie).then(|| id.to_string());
    }
    if is_generic_exe_stem(stem) {
        return None;
    }
    let mut owners: Vec<&crate::protocol::LibraryWatchGameEntry> = games
        .iter()
        .filter(|game| game.exe_stems.iter().any(|s| s.eq_ignore_ascii_case(stem)))
        .collect();
    owners.sort_by(|a, b| a.game_id.cmp(&b.game_id));
    owners.dedup_by(|a, b| a.game_id == b.game_id);
    match owners.as_slice() {
        [only] if only.install_dir_prefixes.is_empty() => Some(only.game_id.clone()),
        _ => None,
    }
}
