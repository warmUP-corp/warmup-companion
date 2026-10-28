use super::*;
use crate::app_profiles::{self as ap, Profile, BUTTONS};

pub(super) const APPS_TAB: usize = 4;
const ROW_H: f32 = 59.0;
const LIST_Y: f32 = CONTENT_Y + ROW_H + ROW_GAP;
const LIST_STEP: f32 = ROW_H + ROW_GAP;
const APPS_Y: f32 = LIST_Y + LIST_STEP;
const MAX_ROWS: usize = 5;
const TILE_W: f32 = (CONTENT_W - ROW_GAP) / 2.0;
const TILE_H: f32 = 48.0;
const TILE_STEP: f32 = TILE_H + ROW_GAP;
const DLG_X: f32 = 142.0;
const DLG_W: f32 = 460.0;
const DLG_IN: f32 = DLG_W - 48.0;
const OPTION_STEP: f32 = 48.0;
const PICK_STEP: f32 = 56.0;
const MAX_PICKS: usize = 5;
const SEARCH_Y: f32 = 86.0;
const PICKS_LABEL_Y: f32 = 150.0;
const PICKS_Y: f32 = 173.0;
const GLYPHS: [usize; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 10, 11, 12, 13, 14, 15];
const CHIP_W: f32 = (DLG_IN - 24.0) / 4.0;
const G_A: &[usize] = &[0];
const G_B: &[usize] = &[1];
const G_Y: &[usize] = &[3];
const G_SHOULDERS: &[usize] = &[4, 5];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComboFocus {
    Chip(usize),
    Key,
    Cancel,
    Use,
}

pub(super) enum Dialog {
    Action {
        button: usize,
        focus: usize,
    },
    Combo {
        button: usize,
        mods: [bool; 4],
        key: usize,
        focus: ComboFocus,
    },
    Add {
        focus: usize,
        first: usize,
        query: String,
        typing: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    App(usize),
    Typed,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) on: bool,
    pub(super) profiles: Vec<Profile>,
    saved_on: bool,
    saved: Vec<Profile>,
    pub(super) editing: Option<usize>,
    pub(super) dialog: Option<Dialog>,
    pub(super) blacklist: Vec<String>,
    saved_blacklist: Vec<String>,
    blacklist_open: bool,
    first: usize,
    wheel: i32,
}

static INSTALLED: OnceLock<Vec<(String, String)>> = OnceLock::new();
static SCAN_STARTED: AtomicBool = AtomicBool::new(false);
static SCAN_DONE: AtomicBool = AtomicBool::new(false);

pub(super) fn prewarm() {
    if SCAN_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("apps-scan".into())
        .spawn(|| {
            let _ = INSTALLED.set(installed_apps());
            SCAN_DONE.store(true, Ordering::SeqCst);
        });
    if spawned.is_err() {
        SCAN_STARTED.store(false, Ordering::SeqCst);
    }
}

pub(super) fn take_scan_done() -> bool {
    SCAN_DONE.swap(false, Ordering::SeqCst)
}

#[derive(Clone, Copy)]
enum Opt {
    Default,
    Preset(&'static str, &'static str),
    Custom,
}

fn settings_text() -> String {
    crate::config::settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
}

pub(super) fn load() -> State {
    let text = settings_text();
    let (on, profiles) = ap::load(&text);
    let blacklist = crate::game_list::parse_blacklist(&text);
    State {
        on,
        saved_on: on,
        saved: profiles.clone(),
        profiles,
        saved_blacklist: blacklist.clone(),
        blacklist,
        ..State::default()
    }
}

pub(super) fn reload(st: &mut State) {
    *st = load();
}

pub(super) fn dirty(st: &State) -> bool {
    st.on != st.saved_on || st.profiles != st.saved || st.blacklist != st.saved_blacklist
}

pub(super) fn apply(st: &mut State) {
    if dirty(st) {
        if let Err(e) =
            crate::config::replace_profile_settings(&ap::settings_lines(st.on, &st.profiles))
        {
            crate::install::log_line(&format!("controller center: app profiles: {e}"));
        }
    }
    if st.blacklist != st.saved_blacklist {
        if let Err(e) = crate::config::set_gamepad_setting("game_blacklist", &st.blacklist.join(", ")) {
            crate::install::log_line(&format!("controller center: blacklist: {e}"));
        }
    }
    reload(st);
}

pub(super) fn enter(st: &mut State) {
    st.editing = None;
    st.dialog = None;
    st.blacklist_open = false;
    st.first = 0;
    prewarm();
}

pub(super) fn leave(ui: &mut Ui) {
    if typing(ui) {
        end_typing(ui);
    }
    ui.apps.editing = None;
    ui.apps.dialog = None;
    ui.apps.blacklist_open = false;
}

fn installed(_st: &State) -> &'static [(String, String)] {
    INSTALLED.get().map_or(&[], Vec::as_slice)
}

fn profile_index(st: &State, exe: &str) -> Option<usize> {
    st.profiles
        .iter()
        .position(|p| p.exes.iter().any(|e| e.eq_ignore_ascii_case(exe)))
}

fn taken(st: &State, exe: &str) -> bool {
    if st.blacklist_open {
        st.blacklist.iter().any(|e| e.eq_ignore_ascii_case(exe))
    } else {
        profile_index(st, exe).is_some()
    }
}

fn installed_name(st: &State, exe: &str) -> String {
    installed(st)
        .iter()
        .find(|(_, e)| e.eq_ignore_ascii_case(exe))
        .map_or_else(|| exe.to_string(), |(n, _)| n.clone())
}

fn name(st: &State, p: &Profile) -> String {
    if !ap::has_builtin(&p.exes[0]) {
        if let Some((n, _)) = installed(st)
            .iter()
            .find(|(_, e)| e.eq_ignore_ascii_case(&p.exes[0]))
        {
            return n.clone();
        }
    }
    p.name()
}

fn default_action(i: usize) -> &'static str {
    match BUTTONS[i] {
        "A" => "Left click",
        "B" => "Right click",
        "SELECT" | "START" => "Enter",
        _ => "Nothing",
    }
}

fn button_name(ps: bool, i: usize) -> &'static str {
    const XBOX: [&str; 14] = [
        "A", "B", "X", "Y", "LB", "RB", "LT", "RT", "View", "Menu", "Left", "Right", "Up", "Down",
    ];
    const PS: [&str; 10] = [
        "Cross", "Circle", "Square", "Triangle", "L1", "R1", "L2", "R2", "Share", "Options",
    ];
    if ps && i < PS.len() {
        PS[i]
    } else {
        XBOX[i]
    }
}

fn options(p: &Profile) -> Vec<Opt> {
    let presets = ap::presets_for(&p.exes[0]);
    std::iter::once(Opt::Default)
        .chain(presets.iter().map(|(l, c)| Opt::Preset(l, c)))
        .chain(std::iter::once(Opt::Custom))
        .collect()
}

fn selected_option(p: &Profile, button: usize) -> usize {
    let opts = options(p);
    match &p.map[button] {
        None => 0,
        Some(c) => opts
            .iter()
            .position(|o| matches!(o, Opt::Preset(_, pc) if pc.eq_ignore_ascii_case(c)))
            .unwrap_or(opts.len() - 1),
    }
}

fn editing_profile(st: &State) -> Option<&Profile> {
    st.editing.and_then(|i| st.profiles.get(i))
}

pub(super) fn rows(ui: &Ui) -> Vec<(Target, Rect)> {
    let st = &ui.apps;
    let mut out = Vec::new();
    if st.editing.is_some() {
        out.push((
            Target::ProfileHeader,
            rect(CONTENT_X, CONTENT_Y, CONTENT_W, ROW_H),
        ));
        for i in 0..BUTTONS.len() {
            out.push((Target::Map(i), map_rect(i)));
        }
        return out;
    }
    let (top, len, row): (f32, usize, fn(usize) -> Target) = if st.blacklist_open {
        out.push((
            Target::ProfileHeader,
            rect(CONTENT_X, CONTENT_Y, CONTENT_W, ROW_H),
        ));
        (LIST_Y, st.blacklist.len(), Target::Blacklisted)
    } else {
        out.push((
            Target::AppsToggle,
            rect(CONTENT_X, CONTENT_Y, CONTENT_W, ROW_H),
        ));
        out.push((Target::Blacklist, rect(CONTENT_X, LIST_Y, CONTENT_W, ROW_H)));
        (APPS_Y, st.profiles.len(), Target::App)
    };
    let shown = len.saturating_sub(st.first).min(MAX_ROWS);
    for slot in 0..shown {
        out.push((
            row(st.first + slot),
            rect(CONTENT_X, top + slot as f32 * LIST_STEP, CONTENT_W, ROW_H),
        ));
    }
    out.push((
        Target::AddApp,
        rect(CONTENT_X, top + shown as f32 * LIST_STEP, CONTENT_W, TILE_H),
    ));
    out
}

fn map_rect(i: usize) -> Rect {
    rect(
        CONTENT_X + (i % 2) as f32 * (TILE_W + ROW_GAP),
        LIST_Y + (i / 2) as f32 * TILE_STEP,
        TILE_W,
        TILE_H,
    )
}

pub(super) fn order(ui: &Ui) -> Vec<Target> {
    let st = &ui.apps;
    let mut out = Vec::new();
    if st.editing.is_some() {
        out.push(Target::ProfileHeader);
        out.extend((0..BUTTONS.len()).map(Target::Map));
    } else if st.blacklist_open {
        out.push(Target::ProfileHeader);
        out.extend((0..st.blacklist.len()).map(Target::Blacklisted));
        out.push(Target::AddApp);
    } else {
        out.push(Target::AppsToggle);
        out.push(Target::Blacklist);
        out.extend((0..st.profiles.len()).map(Target::App));
        out.push(Target::AddApp);
    }
    out.extend([Target::Cancel, Target::Apply]);
    out
}

pub(super) fn keep_visible(ui: &mut Ui) {
    if let Target::App(i) | Target::Blacklisted(i) = ui.focus {
        let st = &mut ui.apps;
        if i < st.first {
            st.first = i;
        } else if i >= st.first + MAX_ROWS {
            st.first = i + 1 - MAX_ROWS;
        }
    }
}

pub(super) fn navigate(ui: &mut Ui, nav: Nav) -> bool {
    if ui.tab != APPS_TAB {
        return false;
    }
    if ui.apps.dialog.is_some() {
        dialog_nav(ui, nav);
        return true;
    }
    let Target::Map(i) = ui.focus else {
        return false;
    };
    let n = BUTTONS.len();
    ui.focus = match nav {
        Nav::Left if i % 2 == 1 => Target::Map(i - 1),
        Nav::Right if i % 2 == 0 && i + 1 < n => Target::Map(i + 1),
        Nav::Up if i >= 2 => Target::Map(i - 2),
        Nav::Up => Target::ProfileHeader,
        Nav::Down if i + 2 < n => Target::Map(i + 2),
        Nav::Down => Target::Cancel,
        _ => ui.focus,
    };
    true
}

pub(super) fn activate(ui: &mut Ui, target: Target) {
    let st = &mut ui.apps;
    match target {
        Target::AppsToggle => st.on = !st.on,
        Target::App(i) => {
            st.editing = Some(i);
            ui.focus = Target::ProfileHeader;
        }
        Target::AddApp => open_add(st),
        Target::Blacklist => {
            st.blacklist_open = true;
            st.first = 0;
            ui.focus = Target::ProfileHeader;
        }
        Target::Blacklisted(i) => {
            if i < st.blacklist.len() {
                st.blacklist.remove(i);
            }
            ui.focus = match st.blacklist.len() {
                0 => Target::AddApp,
                n => Target::Blacklisted(i.min(n - 1)),
            };
            st.first = st.first.min(st.blacklist.len().saturating_sub(MAX_ROWS));
        }
        Target::ProfileHeader if st.blacklist_open => {}
        Target::ProfileHeader => {
            if let Some(p) = st.editing.and_then(|i| st.profiles.get_mut(i)) {
                p.enabled = !p.enabled;
            }
        }
        Target::Map(button) => {
            if let Some(p) = editing_profile(st) {
                let focus = selected_option(p, button);
                st.dialog = Some(Dialog::Action { button, focus });
            }
        }
        _ => {}
    }
}

fn open_add(st: &mut State) {
    prewarm();
    st.dialog = Some(Dialog::Add {
        focus: 0,
        first: 0,
        query: String::new(),
        typing: false,
    });
}

pub(super) fn back(ui: &mut Ui) -> bool {
    if ui.tab != APPS_TAB {
        return false;
    }
    let st = &mut ui.apps;
    match st.dialog.take() {
        Some(Dialog::Combo { button, .. }) => {
            let focus = editing_profile(st).map_or(0, |p| options(p).len() - 1);
            st.dialog = Some(Dialog::Action { button, focus });
            true
        }
        Some(Dialog::Add { query, typing, .. }) => {
            if typing || !query.is_empty() {
                st.dialog = Some(Dialog::Add {
                    focus: 0,
                    first: 0,
                    query: String::new(),
                    typing,
                });
                if typing {
                    end_typing(ui);
                }
            }
            true
        }
        Some(_) => true,
        None if st.blacklist_open => {
            st.blacklist_open = false;
            st.first = 0;
            ui.focus = Target::Blacklist;
            true
        }
        None => match st.editing.take() {
            Some(i) => {
                ui.focus = Target::App(i);
                keep_visible(ui);
                true
            }
            None => false,
        },
    }
}

pub(super) fn pad(ui: &mut Ui, button: &str) -> bool {
    if ui.tab != APPS_TAB {
        return false;
    }
    let modal = ui.apps.dialog.is_some();
    match button {
        "B" => back(ui),
        "A" if modal => {
            dialog_activate(ui);
            true
        }
        "LB" | "RB" if matches!(ui.apps.dialog, Some(Dialog::Add { .. })) => {
            page_add(ui, if button == "LB" { -1 } else { 1 });
            true
        }
        "LB" | "RB" => modal || ui.apps.editing.is_some() || ui.apps.blacklist_open,
        "Y" => modal,
        _ => modal && pad_nav(button).is_none(),
    }
}

pub(super) fn reset(ui: &mut Ui) {
    let st = &mut ui.apps;
    if st.dialog.is_some() {
        return;
    }
    if st.blacklist_open {
        st.blacklist.clear();
        st.first = 0;
        ui.focus = Target::AddApp;
        return;
    }
    match st.editing {
        Some(i) if ap::has_builtin(&st.profiles[i].exes[0]) => {
            st.profiles[i].map = st.profiles[i].builtin_map();
        }
        Some(i) => {
            let n = name(st, &st.profiles[i]);
            ui.confirm = Some((Pending::RemoveApp(n), false));
        }
        None => {
            st.on = true;
            st.profiles = ap::builtin_profiles();
            st.first = 0;
            ui.focus = Target::AppsToggle;
        }
    }
}

pub(super) fn remove_editing(ui: &mut Ui) {
    let st = &mut ui.apps;
    let Some(i) = st.editing.take() else {
        return;
    };
    st.profiles.remove(i);
    ui.focus = if st.profiles.is_empty() {
        Target::AddApp
    } else {
        Target::App(i.min(st.profiles.len() - 1))
    };
    st.first = 0;
    keep_visible(ui);
}

fn add_app(ui: &mut Ui, exe: &str) {
    let st = &mut ui.apps;
    if st.blacklist_open {
        let exe = exe.to_ascii_lowercase();
        let i = match st.blacklist.iter().position(|e| *e == exe) {
            Some(i) => i,
            None => {
                st.blacklist.push(exe);
                st.blacklist.len() - 1
            }
        };
        st.dialog = None;
        ui.focus = Target::Blacklisted(i);
        keep_visible(ui);
        return;
    }
    let i = match profile_index(st, exe) {
        Some(i) => i,
        None => {
            st.profiles.push(ap::new_profile(exe));
            st.profiles.len() - 1
        }
    };
    st.dialog = None;
    st.editing = Some(i);
    ui.focus = Target::ProfileHeader;
}

fn query(st: &State) -> &str {
    match &st.dialog {
        Some(Dialog::Add { query, .. }) => query.trim(),
        _ => "",
    }
}

fn picks(st: &State) -> Vec<Pick> {
    let q = query(st);
    let needle = q.to_lowercase();
    let list = installed(st);
    let mut out: Vec<Pick> = list
        .iter()
        .enumerate()
        .filter(|(_, (n, e))| {
            needle.is_empty()
                || n.to_lowercase().contains(&needle)
                || e.to_lowercase().contains(&needle)
        })
        .map(|(i, _)| Pick::App(i))
        .collect();
    if !q.is_empty() {
        let exe = program_name(q);
        if !list.iter().any(|(_, e)| e.eq_ignore_ascii_case(&exe)) {
            out.push(Pick::Typed);
        }
    }
    out
}

fn pick_exe(st: &State, pick: Pick) -> String {
    match pick {
        Pick::App(i) => installed(st)[i].1.clone(),
        Pick::Typed => program_name(query(st)),
    }
}

fn selectable(st: &State, picks: &[Pick], k: usize) -> bool {
    k == 0 || picks.get(k - 1).is_some_and(|p| !taken(st, &pick_exe(st, *p)))
}

fn step_add(st: &State, from: usize, delta: i32) -> usize {
    let picks = picks(st);
    let last = picks.len() as i64;
    let target = (from as i64 + delta as i64).clamp(0, last);
    let dir = delta.signum() as i64;
    let mut i = target;
    while (0..=last).contains(&i) {
        if i != from as i64 && selectable(st, &picks, i as usize) {
            return i as usize;
        }
        i += dir;
    }
    let mut i = target - dir;
    while (i - from as i64) * dir > 0 {
        if selectable(st, &picks, i as usize) {
            return i as usize;
        }
        i -= dir;
    }
    from
}

fn set_add_focus(st: &mut State, to: usize) {
    if let Some(Dialog::Add { focus, first, .. }) = &mut st.dialog {
        *focus = to;
        match to.checked_sub(1) {
            None => *first = 0,
            Some(slot) if slot < *first => *first = slot,
            Some(slot) if slot >= *first + MAX_PICKS => *first = slot + 1 - MAX_PICKS,
            Some(_) => {}
        }
    }
}

fn edit_query(st: &mut State, edit: impl FnOnce(&mut String)) {
    if let Some(Dialog::Add {
        focus,
        first,
        query,
        ..
    }) = &mut st.dialog
    {
        let before = query.len();
        edit(query);
        if query.len() != before {
            *focus = 0;
            *first = 0;
        }
    }
}

fn commit_pick(ui: &mut Ui, k: usize) {
    let st = &ui.apps;
    let picks = picks(st);
    if k == 0 || !selectable(st, &picks, k) {
        return;
    }
    let exe = pick_exe(st, picks[k - 1]);
    if typing(ui) {
        end_typing(ui);
    }
    add_app(ui, &exe);
}

fn typed_enter(ui: &mut Ui) {
    let Some(Dialog::Add { focus, .. }) = ui.apps.dialog else {
        return;
    };
    if focus > 0 {
        commit_pick(ui, focus);
        return;
    }
    let st = &ui.apps;
    let picks = picks(st);
    let open: Vec<usize> = (1..=picks.len())
        .filter(|k| selectable(st, &picks, *k))
        .collect();
    let apps: Vec<usize> = open
        .iter()
        .copied()
        .filter(|k| matches!(picks[k - 1], Pick::App(_)))
        .collect();
    let typed = open.iter().copied().find(|k| picks[k - 1] == Pick::Typed);
    match (apps.as_slice(), typed) {
        (&[k], _) | (&[], Some(k)) => commit_pick(ui, k),
        (&[], None) => {}
        (&[k, ..], _) => {
            end_typing(ui);
            set_add_focus(&mut ui.apps, k);
        }
    }
}

pub(super) fn wheel(ui: &mut Ui, delta: i32) -> bool {
    if ui.tab != APPS_TAB {
        return false;
    }
    let st = &mut ui.apps;
    st.wheel += delta;
    let steps = st.wheel / 120;
    if steps == 0 {
        return false;
    }
    st.wheel -= steps * 120;
    if let Some(Dialog::Add { focus, first, .. }) = st.dialog {
        let picks = picks(st);
        let n = picks.len();
        let top = (first as i32 - steps).clamp(0, n.saturating_sub(MAX_PICKS) as i32) as usize;
        let window = top..(top + MAX_PICKS).min(n);
        let mut to = focus;
        if focus > 0 && !window.contains(&(focus - 1)) {
            let mut candidates: Vec<usize> = window
                .map(|s| s + 1)
                .filter(|k| selectable(st, &picks, *k))
                .collect();
            if focus - 1 > top {
                candidates.reverse();
            }
            to = candidates.first().copied().unwrap_or(focus);
        }
        if let Some(Dialog::Add { focus, first, .. }) = &mut st.dialog {
            *focus = to;
            *first = top;
        }
        return true;
    }
    if st.dialog.is_some() || st.editing.is_some() {
        return false;
    }
    let n = if st.blacklist_open {
        st.blacklist.len()
    } else {
        st.profiles.len()
    };
    st.first = (st.first as i32 - steps).clamp(0, n.saturating_sub(MAX_ROWS) as i32) as usize;
    if let Target::App(i) | Target::Blacklisted(i) = ui.focus {
        let last = (st.first + MAX_ROWS).min(n) - 1;
        let i = i.clamp(st.first, last);
        ui.focus = if st.blacklist_open {
            Target::Blacklisted(i)
        } else {
            Target::App(i)
        };
    }
    true
}

fn page_add(ui: &mut Ui, dir: i32) {
    let st = &mut ui.apps;
    if let Some(Dialog::Add { focus, .. }) = st.dialog {
        let to = step_add(st, focus, dir * MAX_PICKS as i32);
        set_add_focus(st, to);
    }
}

fn dialog_nav(ui: &mut Ui, nav: Nav) {
    let st = &mut ui.apps;
    if let Some(Dialog::Add { focus, .. }) = st.dialog {
        let to = match nav {
            Nav::Up => step_add(st, focus, -1),
            Nav::Down => step_add(st, focus, 1),
            _ => focus,
        };
        set_add_focus(st, to);
        return;
    }
    let opt_count = editing_profile(st).map_or(0, |p| options(p).len());
    match &mut st.dialog {
        Some(Dialog::Action { focus, .. }) => match nav {
            Nav::Up => *focus = focus.saturating_sub(1),
            Nav::Down => *focus = (*focus + 1).min(opt_count.saturating_sub(1)),
            _ => {}
        },
        Some(Dialog::Combo { focus, key, .. }) => {
            let n = ap::KEYS.len();
            *focus = match (*focus, nav) {
                (ComboFocus::Chip(i), Nav::Left) => ComboFocus::Chip(i.saturating_sub(1)),
                (ComboFocus::Chip(i), Nav::Right) => ComboFocus::Chip((i + 1).min(3)),
                (ComboFocus::Chip(_), Nav::Down) => ComboFocus::Key,
                (ComboFocus::Key, Nav::Left) => {
                    *key = (*key + n - 1) % n;
                    ComboFocus::Key
                }
                (ComboFocus::Key, Nav::Right) => {
                    *key = (*key + 1) % n;
                    ComboFocus::Key
                }
                (ComboFocus::Key, Nav::Up) => ComboFocus::Chip(0),
                (ComboFocus::Key, Nav::Down) => ComboFocus::Use,
                (ComboFocus::Cancel | ComboFocus::Use, Nav::Up) => ComboFocus::Key,
                (ComboFocus::Use, Nav::Left) => ComboFocus::Cancel,
                (ComboFocus::Cancel, Nav::Right) => ComboFocus::Use,
                (f, _) => f,
            };
        }
        _ => {}
    }
}

fn combo_spec(mods: &[bool; 4], key: usize) -> String {
    ap::MODIFIERS
        .iter()
        .zip(mods)
        .filter(|(_, on)| **on)
        .map(|(m, _)| *m)
        .chain(std::iter::once(ap::KEYS[key]))
        .collect::<Vec<_>>()
        .join("+")
}

fn combo_from(spec: Option<&str>) -> ([bool; 4], usize) {
    let mut mods = [false; 4];
    let mut key = ap::KEYS.iter().position(|k| *k == "a").unwrap_or(0);
    for part in spec
        .unwrap_or("")
        .split('+')
        .map(|p| p.trim().to_ascii_lowercase())
    {
        if let Some(m) = ap::MODIFIERS.iter().position(|m| *m == part) {
            mods[m] = true;
        } else if let Some(k) = ap::KEYS.iter().position(|k| *k == part) {
            key = k;
        }
    }
    (mods, key)
}

fn dialog_activate(ui: &mut Ui) {
    let st = &mut ui.apps;
    if let Some(Dialog::Add { focus, .. }) = st.dialog {
        if focus == 0 {
            start_typing(ui);
        } else {
            commit_pick(ui, focus);
        }
        return;
    }
    let Some(i) = st.editing else {
        st.dialog = None;
        return;
    };
    match st.dialog {
        Some(Dialog::Action { button, focus }) => match options(&st.profiles[i])[focus] {
            Opt::Default => {
                st.profiles[i].map[button] = None;
                st.dialog = None;
            }
            Opt::Preset(_, combo) => {
                st.profiles[i].map[button] = Some(combo.to_string());
                st.dialog = None;
            }
            Opt::Custom => {
                let (mods, key) = combo_from(st.profiles[i].map[button].as_deref());
                st.dialog = Some(Dialog::Combo {
                    button,
                    mods,
                    key,
                    focus: ComboFocus::Chip(0),
                });
            }
        },
        Some(Dialog::Combo {
            button,
            mods,
            key,
            focus,
        }) => match focus {
            ComboFocus::Chip(c) => {
                if let Some(Dialog::Combo { mods, .. }) = &mut st.dialog {
                    mods[c] = !mods[c];
                }
            }
            ComboFocus::Key => {
                if let Some(Dialog::Combo { focus, .. }) = &mut st.dialog {
                    *focus = ComboFocus::Use;
                }
            }
            ComboFocus::Cancel => {
                back(ui);
            }
            ComboFocus::Use => {
                st.profiles[i].map[button] = Some(combo_spec(&mods, key));
                st.dialog = None;
            }
        },
        _ => {}
    }
}

fn start_typing(ui: &mut Ui) {
    if let Some(Dialog::Add { typing, .. }) = &mut ui.apps.dialog {
        *typing = true;
        OWNS_PAD.store(false, Ordering::SeqCst);
        crate::pipe_server::request_native_vk(true);
    }
}

fn end_typing(ui: &mut Ui) {
    if let Some(Dialog::Add { typing, .. }) = &mut ui.apps.dialog {
        *typing = false;
    }
    if !OWNS_PAD.load(Ordering::SeqCst) {
        crate::pipe_server::request_native_vk(false);
        let _ = take_pad_edges();
        OWNS_PAD.store(true, Ordering::SeqCst);
    }
}

pub(super) fn typing(ui: &Ui) -> bool {
    matches!(ui.apps.dialog, Some(Dialog::Add { typing: true, .. }))
}

pub(super) fn add_open(ui: &Ui) -> bool {
    ui.tab == APPS_TAB && matches!(ui.apps.dialog, Some(Dialog::Add { .. }))
}

pub(super) fn char_input(ui: &mut Ui, ch: char) {
    let Some(Dialog::Add { typing, query, .. }) = &mut ui.apps.dialog else {
        return;
    };
    if !*typing {
        if !ch.is_alphanumeric() {
            return;
        }
        *typing = true;
    }
    let blank = ch.is_whitespace() && query.chars().last().is_none_or(char::is_whitespace);
    if ch.is_control() || blank || "\\/:*?\"<>|=".contains(ch) || query.chars().count() >= 64 {
        return;
    }
    edit_query(&mut ui.apps, |q| q.push(ch));
}

fn held_mods() -> [bool; 4] {
    let down = |k: u16| unsafe { GetKeyState(k as i32) } < 0;
    [
        down(VK_CONTROL.0),
        down(VK_SHIFT.0),
        down(0x12),
        down(0x5B) || down(0x5C),
    ]
}

fn vk_key(vk: u16, modified: bool) -> Option<&'static str> {
    let name = match vk {
        0x25 if modified => "left",
        0x26 if modified => "up",
        0x27 if modified => "right",
        0x28 if modified => "down",
        0x0D if modified => "enter",
        0x1B if modified => "esc",
        0x20 if modified => "space",
        0x41..=0x5A | 0x30..=0x39 => {
            let c = (vk as u8 as char).to_ascii_lowercase();
            return ap::KEYS
                .iter()
                .copied()
                .find(|k| k.len() == 1 && k.starts_with(c));
        }
        0x08 => "backspace",
        0x09 => "tab",
        0x21 => "pageup",
        0x22 => "pagedown",
        0x23 => "end",
        0x24 => "home",
        0x74 => "f5",
        0x7A => "f11",
        _ => return None,
    };
    Some(name)
}

pub(super) fn records_keys(ui: &Ui) -> bool {
    ui.tab == APPS_TAB
        && matches!(
            ui.apps.dialog,
            Some(Dialog::Action { .. } | Dialog::Combo { .. })
        )
}

pub(super) fn key_down(ui: &mut Ui, vk: u16) -> Option<After> {
    if ui.tab != APPS_TAB {
        return None;
    }
    if typing(ui) {
        match vk {
            v if v == VK_RETURN.0 => typed_enter(ui),
            v if v == VK_ESCAPE.0 => {
                back(ui);
            }
            v if v == VK_UP.0 => dialog_nav(ui, Nav::Up),
            v if v == VK_DOWN.0 => dialog_nav(ui, Nav::Down),
            0x08 => edit_query(&mut ui.apps, |q| {
                q.pop();
            }),
            _ => {}
        }
        return Some(After::Stay);
    }
    if records_keys(ui) {
        let mods = held_mods();
        if let Some(name) = vk_key(vk, mods.iter().any(|m| *m)) {
            let key = ap::KEYS.iter().position(|k| *k == name).unwrap_or(0);
            let button = match ui.apps.dialog {
                Some(Dialog::Action { button, .. } | Dialog::Combo { button, .. }) => button,
                _ => 0,
            };
            ui.apps.dialog = Some(Dialog::Combo {
                button,
                mods,
                key,
                focus: ComboFocus::Use,
            });
            return Some(After::Stay);
        }
    }
    if ui.apps.dialog.is_some() {
        match vk {
            v if v == VK_ESCAPE.0 => {
                back(ui);
            }
            v if v == VK_RETURN.0 || v == VK_SPACE.0 => dialog_activate(ui),
            v if v == VK_UP.0 => dialog_nav(ui, Nav::Up),
            v if v == VK_DOWN.0 => dialog_nav(ui, Nav::Down),
            v if v == VK_LEFT.0 => dialog_nav(ui, Nav::Left),
            v if v == VK_RIGHT.0 => dialog_nav(ui, Nav::Right),
            _ => {}
        }
        return Some(After::Stay);
    }
    if vk == VK_ESCAPE.0 && back(ui) {
        return Some(After::Stay);
    }
    None
}

pub(super) fn hints(ui: &Ui) -> Vec<(&'static [usize], &'static str)> {
    let st = &ui.apps;
    match &st.dialog {
        Some(Dialog::Action { .. }) => vec![(G_A, "Choose"), (G_B, "Cancel")],
        Some(Dialog::Combo { focus, .. }) => vec![
            (
                G_A,
                match focus {
                    ComboFocus::Chip(_) => "Toggle",
                    _ => "Select",
                },
            ),
            (G_B, "Cancel"),
        ],
        Some(Dialog::Add { typing: true, .. }) => vec![],
        Some(Dialog::Add { focus, query, .. }) => vec![
            (G_A, if *focus == 0 { "Search" } else { "Add" }),
            (G_B, if query.is_empty() { "Cancel" } else { "Clear" }),
            (G_SHOULDERS, "Page"),
        ],
        None if st.blacklist_open => vec![(G_A, "Remove"), (G_B, "Back"), (G_Y, "Clear")],
        None => match editing_profile(st) {
            Some(p) => vec![
                (G_A, "Change"),
                (G_B, "Back"),
                (
                    G_Y,
                    if ap::has_builtin(&p.exes[0]) {
                        "Reset to built-in"
                    } else {
                        "Remove app"
                    },
                ),
            ],
            None => vec![
                (G_A, "Open"),
                (G_B, "Back"),
                (G_SHOULDERS, "Tabs"),
                (G_Y, "Reset"),
            ],
        },
    }
}

pub(super) fn click_page(ui: &mut Ui, target: Target, r: Rect, x: f32) {
    if target == Target::ProfileHeader && x < r.x + 40.0 {
        back(ui);
    } else {
        activate(ui, target);
    }
}

pub(super) fn click_dialog(ui: &mut Ui, x: f32, y: f32) {
    let Some(card) = dialog_card(ui) else {
        return;
    };
    if !card.contains(x, y) {
        return;
    }
    if let Some(Dialog::Add { first, .. }) = ui.apps.dialog {
        if search_rect(card).contains(x, y) {
            set_add_focus(&mut ui.apps, 0);
            start_typing(ui);
            return;
        }
        let n = picks(&ui.apps).len();
        let shown = n.saturating_sub(first).min(MAX_PICKS);
        if let Some(s) = (0..shown).find(|s| pick_rect(card, *s).contains(x, y)) {
            commit_pick(ui, first + s + 1);
        }
        return;
    }
    let n_opts = editing_profile(&ui.apps).map_or(0, |p| options(p).len());
    match &mut ui.apps.dialog {
        Some(Dialog::Action { focus, .. }) => {
            let Some(i) = (0..n_opts).find(|i| option_rect(card, *i).contains(x, y)) else {
                return;
            };
            *focus = i;
        }
        Some(Dialog::Combo { focus, key, .. }) => {
            let n = ap::KEYS.len();
            let stepper = stepper_rect(card);
            if let Some(c) = (0..4).find(|c| chip_rect(card, *c).contains(x, y)) {
                *focus = ComboFocus::Chip(c);
            } else if stepper.contains(x, y) {
                if x < stepper.x + 60.0 {
                    *key = (*key + n - 1) % n;
                } else if x > stepper.x + stepper.w - 60.0 {
                    *key = (*key + 1) % n;
                }
                *focus = ComboFocus::Key;
                return;
            } else if combo_button(card, false).contains(x, y) {
                *focus = ComboFocus::Cancel;
            } else if combo_button(card, true).contains(x, y) {
                *focus = ComboFocus::Use;
            } else {
                return;
            }
        }
        _ => return,
    }
    dialog_activate(ui);
}

fn dialog_height(ui: &Ui) -> f32 {
    let st = &ui.apps;
    match &st.dialog {
        Some(Dialog::Action { .. }) => {
            let n = editing_profile(st).map_or(0, |p| options(p).len()) as f32;
            106.0 + n * OPTION_STEP - 4.0 + 24.0
        }
        Some(Dialog::Combo { .. }) => 418.0,
        Some(Dialog::Add { .. }) => PICKS_Y + MAX_PICKS as f32 * PICK_STEP + 20.0 + 24.0,
        None => 0.0,
    }
}

fn dialog_card(ui: &Ui) -> Option<Rect> {
    ui.apps.dialog.as_ref()?;
    let h = dialog_height(ui);
    Some(rect(DLG_X, ((HINT_Y - h) / 2.0).round(), DLG_W, h))
}

fn option_rect(card: Rect, i: usize) -> Rect {
    rect(
        card.x + 24.0,
        card.y + 106.0 + i as f32 * OPTION_STEP,
        DLG_IN,
        44.0,
    )
}

fn chip_rect(card: Rect, i: usize) -> Rect {
    rect(
        card.x + 24.0 + i as f32 * (CHIP_W + 8.0),
        card.y + 125.0,
        CHIP_W,
        44.0,
    )
}

fn stepper_rect(card: Rect) -> Rect {
    rect(card.x + 24.0, card.y + 206.0, DLG_IN, 64.0)
}

fn combo_button(card: Rect, ok: bool) -> Rect {
    let x = card.x + card.w - 24.0 - 110.0 - if ok { 0.0 } else { 120.0 };
    rect(x, card.y + 350.0, 110.0, 44.0)
}

fn pick_rect(card: Rect, slot: usize) -> Rect {
    rect(
        card.x + 24.0,
        card.y + PICKS_Y + slot as f32 * PICK_STEP,
        DLG_IN,
        52.0,
    )
}

fn search_rect(card: Rect) -> Rect {
    rect(card.x + 24.0, card.y + SEARCH_Y, DLG_IN, 52.0)
}

unsafe fn keycaps_width(g: &mut Gfx, caps: &[String]) -> f32 {
    let plus = g.measure("+", 12.0, false);
    caps.iter()
        .map(|c| g.measure(c, 12.0, true) + 14.0)
        .sum::<f32>()
        + caps.len().saturating_sub(1) as f32 * (plus + 8.0)
}

unsafe fn draw_keycaps(g: &mut Gfx, t: &Theme, caps: &[String], x: f32, cy: f32) {
    let plus = g.measure("+", 12.0, false);
    let mut x = x;
    for (j, c) in caps.iter().enumerate() {
        if j > 0 {
            x += 4.0;
            g.text(
                "+",
                rect(x, cy - 12.0, plus + 2.0, 24.0),
                12.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Left,
            );
            x += plus + 4.0;
        }
        let w = g.measure(c, 12.0, true) + 14.0;
        let r = rect(x, cy - 12.0, w, 24.0);
        g.fill(r, 6.0, t.bg, 1.0);
        g.ring(r, 6.0, 1.0, t.line, t.line_alpha);
        g.text(c, r, 12.0, true, t.text, 1.0, Align::Center);
        x += w;
    }
}

unsafe fn fit(g: &mut Gfx, text: &str, w: f32, size: f32, bold: bool) -> String {
    ellipsize(text, w, |x| g.measure(x, size, bold))
}

unsafe fn two_lines(g: &mut Gfx, t: &Theme, r: Rect, title: &str, caption: &str, dim: bool) {
    let (c, a) = if dim {
        (t.dim, DIM_ALPHA)
    } else {
        (t.text, 1.0)
    };
    let fitted = fit(g, title, r.w, 14.0, false);
    g.text(
        &fitted,
        rect(r.x, r.y + 12.0, r.w, 17.0),
        14.0,
        false,
        c,
        a,
        Align::Left,
    );
    let fitted = fit(g, caption, r.w, 12.0, false);
    g.text(
        &fitted,
        rect(r.x, r.y + 32.0, r.w, 15.0),
        12.0,
        false,
        t.dim,
        DIM_ALPHA,
        Align::Left,
    );
}

fn browser_caption() -> String {
    format!(
        "Built-in \u{00B7} Chrome, Edge, Firefox +{}",
        ap::BROWSERS.len() - 3
    )
}

pub(super) unsafe fn draw_row(ui: &Ui, g: &mut Gfx, target: Target, r: Rect) {
    let t = ui.theme;
    let s = ui.scale;
    let st = &ui.apps;
    let focused = ui.focus == target;
    g.fill(r, 12.0, if focused { t.hi } else { t.tile }, 1.0);
    if focused {
        g.ring(r, 12.0, 2.0, t.ring, 1.0);
    }
    match target {
        Target::AppsToggle => {
            two_lines(
                g,
                &t,
                rect(r.x + ROW_PAD, r.y, r.w - 2.0 * ROW_PAD - 62.0, r.h),
                "Per-app profiles",
                "Buttons send app shortcuts while that app is in front",
                false,
            );
            draw_toggle(g, &t, toggle_rect(r), st.on);
        }
        Target::Blacklist => {
            two_lines(
                g,
                &t,
                rect(r.x + ROW_PAD, r.y, r.w - 2.0 * ROW_PAD - 170.0, r.h),
                "Blacklist",
                "Programs that never count as a game",
                false,
            );
            if is_new(ui, "blacklist") {
                let x = r.x + ROW_PAD + g.measure("Blacklist", 14.0, false) + 8.0;
                draw_new_chip(g, &t, x, r.y + 20.5);
            }
            let chev = rect(r.x + r.w - ROW_PAD - 18.0, r.y + 20.5, 18.0, 18.0);
            g.icon(Icon::ChevronRight, chev, t.dim, DIM_ALPHA, s);
            let n = st.blacklist.len();
            g.text(
                &format!("{n} program{}", if n == 1 { "" } else { "s" }),
                rect(chev.x - 8.0 - 140.0, r.y, 140.0, r.h),
                13.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Right,
            );
        }
        Target::Blacklisted(i) => {
            let Some(exe) = st.blacklist.get(i) else {
                return;
            };
            let av = rect(r.x + 16.0, r.y + 13.0, 33.0, 33.0);
            g.fill(av, 10.0, t.bg, 1.0);
            g.icon(
                Icon::AppWindow,
                rect(av.x + 7.5, av.y + 7.5, 18.0, 18.0),
                t.accent,
                1.0,
                s,
            );
            two_lines(
                g,
                &t,
                rect(r.x + 61.0, r.y, 340.0, r.h),
                &installed_name(st, exe),
                exe,
                false,
            );
            g.icon(
                Icon::Trash,
                rect(r.x + r.w - ROW_PAD - 18.0, r.y + 20.5, 18.0, 18.0),
                t.dim,
                DIM_ALPHA,
                s,
            );
        }
        Target::App(i) => {
            let p = &st.profiles[i];
            let av = rect(r.x + 16.0, r.y + 13.0, 33.0, 33.0);
            g.fill(av, 10.0, t.bg, 1.0);
            let (ic, ia) = if p.enabled {
                (t.accent, 1.0)
            } else {
                (t.dim, DIM_ALPHA)
            };
            g.icon(
                if p.browsers {
                    Icon::Globe
                } else {
                    Icon::AppWindow
                },
                rect(av.x + 7.5, av.y + 7.5, 18.0, 18.0),
                ic,
                ia,
                s,
            );
            let caption = if p.browsers {
                browser_caption()
            } else if ap::has_builtin(&p.exes[0]) {
                format!("Built-in \u{00B7} {}", p.exes[0])
            } else {
                p.exes[0].clone()
            };
            two_lines(
                g,
                &t,
                rect(r.x + 61.0, r.y, 260.0, r.h),
                &name(st, p),
                &caption,
                !p.enabled,
            );
            let status = if !p.enabled {
                "Off".to_string()
            } else {
                let n = p.mapped();
                format!("{n} button{}", if n == 1 { "" } else { "s" })
            };
            let chev = rect(r.x + r.w - ROW_PAD - 18.0, r.y + 20.5, 18.0, 18.0);
            g.icon(Icon::ChevronRight, chev, t.dim, DIM_ALPHA, s);
            g.text(
                &status,
                rect(chev.x - 8.0 - 140.0, r.y, 140.0, r.h),
                13.0,
                !p.enabled,
                t.dim,
                DIM_ALPHA,
                Align::Right,
            );
        }
        Target::AddApp => {
            let add = if st.blacklist_open { "Add program" } else { "Add app" };
            let lw = g.measure(add, 14.0, false);
            let x0 = r.x + (r.w - (28.0 + lw)) / 2.0;
            g.icon(
                Icon::Plus,
                rect(x0, r.y + 15.0, 18.0, 18.0),
                t.dim,
                DIM_ALPHA,
                s,
            );
            g.text(
                add,
                rect(x0 + 28.0, r.y, lw + 4.0, r.h),
                14.0,
                false,
                t.text,
                1.0,
                Align::Left,
            );
            let len = if st.blacklist_open {
                st.blacklist.len()
            } else {
                st.profiles.len()
            };
            let below = len.saturating_sub(st.first + MAX_ROWS);
            let more = match (below, st.first) {
                (0, 0) => return,
                (0, above) => format!("{above} more above \u{00B7} scroll up"),
                (below, _) => format!("{below} more below \u{00B7} scroll down"),
            };
            let y = r.y + r.h + 8.0;
            let mw = g.measure(&more, 11.0, false);
            let x0 = r.x + (r.w - (20.0 + mw)) / 2.0;
            if below > 0 {
                g.icon(
                    Icon::ChevronDown,
                    rect(x0, y + 3.0, 14.0, 14.0),
                    t.dim,
                    DIM_ALPHA,
                    s,
                );
            }
            g.text(
                &more,
                rect(x0 + 20.0, y, mw + 4.0, 20.0),
                11.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Left,
            );
        }
        Target::ProfileHeader if st.blacklist_open => {
            g.icon(
                Icon::ChevronLeft,
                rect(r.x + 14.0, r.y + 20.5, 18.0, 18.0),
                t.dim,
                DIM_ALPHA,
                s,
            );
            two_lines(
                g,
                &t,
                rect(r.x + 40.0, r.y, 420.0, r.h),
                "Blacklist",
                "Fullscreen windows of these programs never pause the controller",
                false,
            );
        }
        Target::ProfileHeader => {
            let Some(p) = editing_profile(st) else {
                return;
            };
            g.icon(
                Icon::ChevronLeft,
                rect(r.x + 14.0, r.y + 20.5, 18.0, 18.0),
                t.dim,
                DIM_ALPHA,
                s,
            );
            let fitted = fit(g, &name(st, p), 380.0, 14.0, true);
            g.text(
                &fitted,
                rect(r.x + 40.0, r.y + 12.0, 380.0, 17.0),
                14.0,
                true,
                t.text,
                1.0,
                Align::Left,
            );
            let caption = if ap::has_builtin(&p.exes[0]) {
                format!("Built-in profile \u{00B7} {}", {
                    let names: Vec<&str> = p
                        .exes
                        .iter()
                        .map(|b| b.trim_end_matches(".exe"))
                        .collect();
                    names.join(", ")
                })
            } else {
                p.exes[0].clone()
            };
            g.text(
                &caption,
                rect(r.x + 40.0, r.y + 32.0, 380.0, 15.0),
                12.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Left,
            );
            draw_toggle(g, &t, toggle_rect(r), p.enabled);
        }
        Target::Map(i) => {
            let Some(p) = editing_profile(st) else {
                return;
            };
            let ps = ui.live.playstation();
            g.icon(
                Icon::Pad(ps, GLYPHS[i]),
                rect(r.x + 14.0, r.y + 13.0, 22.0, 22.0),
                t.text,
                1.0,
                s,
            );
            match &p.map[i] {
                Some(c) => draw_keycaps(g, &t, &ap::keycaps(c), r.x + 46.0, r.y + 24.0),
                None => g.text(
                    &format!("Default \u{00B7} {}", default_action(i)),
                    rect(r.x + 46.0, r.y, r.w - 60.0, r.h),
                    13.0,
                    false,
                    t.dim,
                    DIM_ALPHA,
                    Align::Left,
                ),
            }
            if ap::has_builtin(&p.exes[0]) && p.map[i] != p.builtin_map()[i] {
                g.circle(r.x + 229.0, r.y + 24.0, 3.0, t.accent);
            }
        }
        _ => {}
    }
}

unsafe fn dialog_frame(g: &mut Gfx, t: &Theme, card: Rect) {
    g.fill(rect(0.0, 0.0, WIN_W, WIN_H), WIN_R, 0, 0.55);
    g.fill(card, 16.0, t.tile, 1.0);
    g.ring(card, 16.0, 1.0, t.line, t.line_alpha);
}

unsafe fn focus_box(g: &mut Gfx, t: &Theme, r: Rect, focused: bool) {
    if focused {
        g.fill(r, 12.0, t.hi, 1.0);
        g.ring(r, 12.0, 2.0, t.ring, 1.0);
    }
}

pub(super) unsafe fn draw_dialog(ui: &Ui, g: &mut Gfx) {
    let Some(card) = dialog_card(ui) else {
        return;
    };
    let t = ui.theme;
    let s = ui.scale;
    let st = &ui.apps;
    let ps = ui.live.playstation();
    dialog_frame(g, &t, card);
    let x = card.x + 24.0;
    let title = |g: &mut Gfx, button: Option<usize>, text: &str| {
        let tx = match button {
            Some(b) => {
                g.icon(
                    Icon::Pad(ps, GLYPHS[b]),
                    rect(x, card.y + 24.0, 22.0, 22.0),
                    t.text,
                    1.0,
                    s,
                );
                x + 32.0
            }
            None => x,
        };
        let fitted = fit(g, text, DLG_IN - (tx - x), 17.0, true);
        g.text(
            &fitted,
            rect(tx, card.y + 24.0, DLG_IN - (tx - x), 22.0),
            17.0,
            true,
            t.text,
            1.0,
            Align::Left,
        );
    };
    let body = |g: &mut Gfx, line: usize, text: &str| {
        g.text(
            text,
            rect(x, card.y + 54.0 + line as f32 * 18.0, DLG_IN, 16.0),
            13.0,
            false,
            t.dim,
            DIM_ALPHA,
            Align::Left,
        );
    };
    let label = |g: &mut Gfx, y: f32, text: &str| {
        g.text(
            text,
            rect(x, card.y + y, DLG_IN, 15.0),
            12.0,
            false,
            t.dim,
            DIM_ALPHA,
            Align::Left,
        );
    };
    match &st.dialog {
        Some(Dialog::Action { button, focus }) => {
            let Some(p) = editing_profile(st) else {
                return;
            };
            let bn = button_name(ps, *button);
            title(g, Some(*button), &format!("{bn} in {}", name(st, p)));
            let who = if p.browsers {
                "a browser".to_string()
            } else {
                name(st, p)
            };
            body(g, 0, &format!("What {bn} sends while {who} is in front."));
            body(g, 1, "Or press the combo on a keyboard.");
            let sel = selected_option(p, *button);
            for (i, opt) in options(p).iter().enumerate() {
                let r = option_rect(card, i);
                focus_box(g, &t, r, i == *focus);
                let chosen = i == sel;
                if chosen {
                    g.icon(
                        Icon::Check,
                        rect(r.x + 12.0, r.y + 14.0, 16.0, 16.0),
                        t.accent,
                        1.0,
                        s,
                    );
                }
                let text = match opt {
                    Opt::Default => "Default",
                    Opt::Preset(l, _) => l,
                    Opt::Custom => "Custom key combo\u{2026}",
                };
                let lw = g.measure(text, 14.0, chosen);
                g.text(
                    text,
                    rect(r.x + 38.0, r.y, lw + 4.0, r.h),
                    14.0,
                    chosen,
                    t.text,
                    1.0,
                    Align::Left,
                );
                let right = r.x + r.w - 12.0;
                match opt {
                    Opt::Default => g.text(
                        &format!("\u{00B7} {}", default_action(*button)),
                        rect(r.x + 38.0 + lw + 6.0, r.y, 200.0, r.h),
                        13.0,
                        false,
                        t.dim,
                        DIM_ALPHA,
                        Align::Left,
                    ),
                    Opt::Preset(_, combo) => {
                        let caps = ap::keycaps(combo);
                        let w = keycaps_width(g, &caps);
                        draw_keycaps(g, &t, &caps, right - w, r.y + 22.0);
                    }
                    Opt::Custom => {
                        g.icon(
                            Icon::ChevronRight,
                            rect(right - 18.0, r.y + 13.0, 18.0, 18.0),
                            t.dim,
                            DIM_ALPHA,
                            s,
                        );
                        if chosen {
                            if let Some(c) = &p.map[*button] {
                                let caps = ap::keycaps(c);
                                let w = keycaps_width(g, &caps);
                                draw_keycaps(g, &t, &caps, right - 26.0 - w, r.y + 22.0);
                            }
                        }
                    }
                }
            }
        }
        Some(Dialog::Combo {
            button,
            mods,
            key,
            focus,
        }) => {
            title(
                g,
                Some(*button),
                &format!("Custom combo for {}", button_name(ps, *button)),
            );
            body(g, 0, "Turn on modifiers, then pick a key.");
            body(g, 1, "Pressing a key on a keyboard also works.");
            label(g, 104.0, "Modifiers");
            for (c, on) in mods.iter().enumerate() {
                let r = chip_rect(card, c);
                let f = *focus == ComboFocus::Chip(c);
                g.fill(
                    r,
                    12.0,
                    if *on {
                        t.accent
                    } else if f {
                        t.hi
                    } else {
                        t.bg
                    },
                    1.0,
                );
                if f {
                    g.ring(r, 12.0, 2.0, t.ring, 1.0);
                }
                g.text(
                    &ap::key_label(ap::MODIFIERS[c]),
                    r,
                    14.0,
                    *on || f,
                    if *on { t.on_accent } else { t.text },
                    1.0,
                    Align::Center,
                );
            }
            label(g, 185.0, "Key");
            let st_r = stepper_rect(card);
            g.fill(st_r, 12.0, t.bg, 1.0);
            if *focus == ComboFocus::Key {
                g.ring(st_r, 12.0, 2.0, t.ring, 1.0);
            }
            g.icon(
                Icon::ChevronLeft,
                rect(st_r.x + 14.0, st_r.y + 22.0, 20.0, 20.0),
                t.dim,
                DIM_ALPHA,
                s,
            );
            g.icon(
                Icon::ChevronRight,
                rect(st_r.x + st_r.w - 34.0, st_r.y + 22.0, 20.0, 20.0),
                t.dim,
                DIM_ALPHA,
                s,
            );
            let kl = ap::key_label(ap::KEYS[*key]);
            let kw = (g.measure(&kl, 20.0, true) + 28.0).max(52.0);
            let kr = rect(st_r.x + (st_r.w - kw) / 2.0, st_r.y + 12.0, kw, 40.0);
            g.fill(kr, 8.0, t.hi, 1.0);
            g.text(&kl, kr, 20.0, true, t.text, 1.0, Align::Center);
            g.text(
                "D-pad left / right steps through letters, digits and special keys.",
                rect(x, card.y + 278.0, DLG_IN, 14.0),
                11.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Left,
            );
            let sends_w = g.measure("Sends", 13.0, false);
            g.text(
                "Sends",
                rect(x, card.y + 306.0, sends_w + 4.0, 24.0),
                13.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Left,
            );
            draw_keycaps(
                g,
                &t,
                &ap::keycaps(&combo_spec(mods, *key)),
                x + sends_w + 8.0,
                card.y + 318.0,
            );
            for (ok, text, fill, ink) in [
                (false, "Cancel", t.hi, t.text),
                (true, "Use", t.accent, t.on_accent),
            ] {
                let r = combo_button(card, ok);
                g.fill(r, 12.0, fill, 1.0);
                if *focus
                    == if ok {
                        ComboFocus::Use
                    } else {
                        ComboFocus::Cancel
                    }
                {
                    g.ring(r, 12.0, 2.0, t.ring, 1.0);
                }
                g.text(text, r, 15.0, true, ink, 1.0, Align::Center);
            }
        }
        Some(Dialog::Add {
            focus,
            first,
            query,
            typing,
        }) => {
            title(
                g,
                None,
                if st.blacklist_open {
                    "Add a program to the blacklist"
                } else {
                    "Add an app"
                },
            );
            body(
                g,
                0,
                if *typing {
                    "Type to search. Enter adds the match, Esc clears."
                } else {
                    "Pick an installed app, or search by name."
                },
            );
            let sr = search_rect(card);
            let av = rect(sr.x + 12.0, sr.y + 10.0, 32.0, 32.0);
            if *typing {
                g.fill(sr, 12.0, t.bg, 1.0);
                if *focus == 0 {
                    g.ring(sr, 12.0, 2.0, t.ring, 1.0);
                } else {
                    g.ring(sr, 12.0, 1.0, t.line, t.line_alpha);
                }
                g.icon(
                    Icon::Keyboard,
                    rect(av.x + 7.0, av.y + 7.0, 18.0, 18.0),
                    t.accent,
                    1.0,
                    s,
                );
                let room = sr.w - 72.0;
                let mut shown_text = query.as_str();
                while g.measure(shown_text, 14.0, false) > room {
                    let mut rest = shown_text.chars();
                    rest.next();
                    shown_text = rest.as_str();
                }
                let (text, c, a) = if query.is_empty() {
                    (TYPE_PLACEHOLDER, t.dim, DIM_ALPHA)
                } else {
                    (shown_text, t.text, 1.0)
                };
                g.text(
                    text,
                    rect(sr.x + 56.0, sr.y, room, sr.h),
                    14.0,
                    false,
                    c,
                    a,
                    Align::Left,
                );
                let cx = if query.is_empty() {
                    sr.x + 55.0
                } else {
                    sr.x + 57.0 + g.measure(shown_text, 14.0, false)
                };
                g.fill(rect(cx, sr.y + 16.0, 1.5, 20.0), 0.0, t.accent, 1.0);
            } else {
                g.fill(sr, 12.0, t.bg, 1.0);
                focus_box(g, &t, sr, *focus == 0);
                g.icon(
                    Icon::Keyboard,
                    rect(av.x + 7.0, av.y + 7.0, 18.0, 18.0),
                    t.dim,
                    DIM_ALPHA,
                    s,
                );
                let tr = rect(sr.x + 56.0, sr.y, sr.w - 68.0, sr.h);
                if query.is_empty() {
                    two_lines(
                        g,
                        &t,
                        tr,
                        "Search apps or type a program name",
                        TYPE_CAPTION,
                        false,
                    );
                } else {
                    two_lines(g, &t, tr, query, "Search \u{00B7} A to edit", false);
                }
            }
            let picks = picks(st);
            let n_apps = picks.iter().filter(|p| matches!(p, Pick::App(_))).count();
            let q = query.trim();
            let heading = if q.is_empty() {
                "Installed apps".to_string()
            } else if INSTALLED.get().is_none() {
                "Finding installed apps\u{2026}".to_string()
            } else if n_apps == 0 {
                "No installed app matches".to_string()
            } else if n_apps == 1 {
                "1 match".to_string()
            } else {
                format!("{n_apps} matches")
            };
            label(g, PICKS_LABEL_Y, &heading);
            if picks.is_empty() {
                g.text(
                    if INSTALLED.get().is_none() {
                        "Finding installed apps\u{2026}"
                    } else {
                        "No installed apps found"
                    },
                    pick_rect(card, 0),
                    13.0,
                    false,
                    t.dim,
                    DIM_ALPHA,
                    Align::Center,
                );
            }
            let shown = picks.len().saturating_sub(*first).min(MAX_PICKS);
            for slot in 0..shown {
                let k = first + slot + 1;
                let pick = picks[k - 1];
                let exe = pick_exe(st, pick);
                let (top, sub, icon) = match pick {
                    Pick::App(i) => (installed(st)[i].0.clone(), exe.clone(), Icon::AppWindow),
                    Pick::Typed => (
                        format!("Use \"{exe}\""),
                        "Program name you typed".to_string(),
                        Icon::Keyboard,
                    ),
                };
                let r = pick_rect(card, slot);
                let has = taken(st, &exe);
                focus_box(g, &t, r, k == *focus);
                let a = if has { 0.5 } else { 1.0 };
                let av = rect(r.x + 12.0, r.y + 10.0, 32.0, 32.0);
                g.fill(av, 10.0, t.bg, a);
                g.icon(
                    icon,
                    rect(av.x + 7.0, av.y + 7.0, 18.0, 18.0),
                    t.accent,
                    a,
                    s,
                );
                let tw = r.w - 68.0 - if has { 112.0 } else { 0.0 };
                let top = fit(g, &top, tw, 14.0, false);
                let sub = fit(g, &sub, tw, 12.0, false);
                g.text(
                    &top,
                    rect(r.x + 56.0, r.y + 9.0, tw, 17.0),
                    14.0,
                    false,
                    t.text,
                    a,
                    Align::Left,
                );
                g.text(
                    &sub,
                    rect(r.x + 56.0, r.y + 28.0, tw, 15.0),
                    12.0,
                    false,
                    t.dim,
                    DIM_ALPHA * a,
                    Align::Left,
                );
                if has {
                    g.text(
                        if st.blacklist_open {
                            "Already listed"
                        } else {
                            "Has a profile"
                        },
                        rect(r.x + r.w - 112.0, r.y, 100.0, r.h),
                        12.0,
                        false,
                        t.dim,
                        DIM_ALPHA,
                        Align::Right,
                    );
                }
            }
            if picks.len() > MAX_PICKS {
                let below = picks.len() - first - shown;
                let more = if below > 0 {
                    format!("{below} more \u{00B7} LB / RB page")
                } else {
                    "LB / RB page".to_string()
                };
                let y = card.y + PICKS_Y + MAX_PICKS as f32 * PICK_STEP;
                let mw = g.measure(&more, 11.0, false);
                let x0 = x + (DLG_IN - (20.0 + mw)) / 2.0;
                if below > 0 {
                    g.icon(
                        Icon::ChevronDown,
                        rect(x0, y + 3.0, 14.0, 14.0),
                        t.dim,
                        DIM_ALPHA,
                        s,
                    );
                }
                g.text(
                    &more,
                    rect(x0 + 20.0, y, mw + 4.0, 20.0),
                    11.0,
                    false,
                    t.dim,
                    DIM_ALPHA,
                    Align::Left,
                );
            }
        }
        None => {}
    }
}
