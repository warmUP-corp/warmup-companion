//! Local prefix prediction (ADR 0001): VK-only context, userland only.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::predict_ngram;

const MIN_PREFIX_LEN: usize = 2;
const MAX_CANDIDATES: usize = 5;
const VISIBLE: usize = 3;

fn new_state() -> PredictState {
    PredictState {
        enabled: false,
        words: Vec::new(),
        partial: String::new(),
        ranked: Vec::new(),
        highlight: 0,
        candidate_engaged: false,
        personal: HashSet::new(),
    }
}

static STATE: std::sync::LazyLock<Mutex<PredictState>> =
    std::sync::LazyLock::new(|| Mutex::new(new_state()));

struct PredictState {
    enabled: bool,
    words: Vec<String>,
    partial: String,
    ranked: Vec<String>,
    highlight: usize,
    /// True after LB/RB cycle while the strip is showing (A may commit).
    candidate_engaged: bool,
    personal: HashSet<String>,
}

/// Single source of truth for the candidate strip: the visible chips, which slot
/// is highlighted, and whether the user engaged the strip with LB/RB (so A may
/// commit). `strip()` returns this; `None` means no strip should show.
pub struct StripState {
    pub visible: [String; VISIBLE],
    pub highlight_slot: usize,
    pub engaged: bool,
}

fn lexicon() -> &'static [&'static str] {
    predict_ngram::lexicon()
}

fn words_with_prefix<'a>(prefix: &'a str) -> impl Iterator<Item = &'a str> {
    let lex = lexicon();
    let start = lex.partition_point(|w| *w < prefix);
    lex[start..]
        .iter()
        .copied()
        .take_while(move |w| w.starts_with(prefix))
}

fn personal_dict_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(base)
            .join("WarmupKeyboard")
            .join("personal.dict"),
    )
}

fn load_personal(into: &mut HashSet<String>) {
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

fn flush_personal(from: &HashSet<String>) {
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

pub fn predictions_enabled() -> bool {
    #[cfg(test)]
    {
        return true;
    }
    #[cfg(not(test))]
    {
        crate::win::surface::input()
            .map(|s| s.is_userland())
            .unwrap_or(false)
    }
}

pub fn reset() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    s.words.clear();
    s.partial.clear();
    s.ranked.clear();
    s.highlight = 0;
    s.candidate_engaged = false;
    s.enabled = predictions_enabled();
    load_personal(&mut s.personal);
}

fn refresh_ranked(s: &mut PredictState) {
    s.ranked.clear();
    s.highlight = 0;
    s.candidate_engaged = false;
    if !s.enabled || s.partial.len() < MIN_PREFIX_LEN {
        return;
    }
    let prefix = s.partial.as_str();
    let prev = s.words.last().and_then(|w| predict_ngram::word_id(w));
    let prev2 = s
        .words
        .get(s.words.len().saturating_sub(2))
        .and_then(|w| predict_ngram::word_id(w));

    let mut scored: Vec<(u32, String)> = Vec::new();
    for word in words_with_prefix(prefix) {
        let Some(id) = predict_ngram::word_id(word) else {
            continue;
        };
        let personal = s.personal.contains(word);
        let score = predict_ngram::rank_score(prev, prev2, id, personal);
        scored.push((score, word.to_string()));
    }
    for word in &s.personal {
        if word.starts_with(prefix) && !scored.iter().any(|(_, w)| w == word) {
            let score = predict_ngram::rank_score(
                prev,
                prev2,
                predict_ngram::word_id(word).unwrap_or(0),
                true,
            );
            scored.push((score, word.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, w) in scored.into_iter().take(MAX_CANDIDATES) {
        s.ranked.push(w);
    }
}

/// The current candidate strip, or `None` when no strip should show. The one
/// query for both rendering and the LB/RB context-swap decision.
pub fn strip() -> Option<StripState> {
    let s = STATE.lock().ok()?;
    if !strip_active_inner(&s) {
        return None;
    }
    let start = viewport_start(s.highlight, s.ranked.len());
    let mut visible = [String::new(), String::new(), String::new()];
    for (i, slot) in visible.iter_mut().enumerate() {
        if let Some(w) = s.ranked.get(start + i) {
            *slot = w.clone();
        }
    }
    let highlight_slot = s.highlight.saturating_sub(start);
    Some(StripState {
        visible,
        highlight_slot,
        engaged: s.candidate_engaged,
    })
}

fn strip_active_inner(s: &PredictState) -> bool {
    s.enabled && s.partial.len() >= MIN_PREFIX_LEN && !s.ranked.is_empty()
}

fn viewport_start(highlight: usize, total: usize) -> usize {
    if total <= VISIBLE {
        return 0;
    }
    if highlight <= 1 {
        0
    } else if highlight >= total.saturating_sub(2) {
        total - VISIBLE
    } else {
        highlight - 1
    }
}

pub fn cycle_next() -> bool {
    let Ok(mut s) = STATE.lock() else {
        return false;
    };
    if s.ranked.is_empty() {
        return false;
    }
    s.highlight = (s.highlight + 1) % s.ranked.len();
    s.candidate_engaged = true;
    true
}

pub fn cycle_prev() -> bool {
    let Ok(mut s) = STATE.lock() else {
        return false;
    };
    if s.ranked.is_empty() {
        return false;
    }
    s.highlight = if s.highlight == 0 {
        s.ranked.len() - 1
    } else {
        s.highlight - 1
    };
    s.candidate_engaged = true;
    true
}

pub fn on_char(c: char) {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    s.enabled = predictions_enabled();
    if !s.enabled {
        return;
    }
    if c.is_ascii_alphabetic() {
        s.partial.push(c.to_ascii_lowercase());
        refresh_ranked(&mut s);
    } else if c.is_ascii_digit() || c == '_' {
        finish_word(&mut s);
        s.partial.push(c);
        refresh_ranked(&mut s);
    } else {
        finish_word(&mut s);
    }
}

pub fn on_backspace() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    if !s.partial.is_empty() {
        s.partial.pop();
        refresh_ranked(&mut s);
    } else if s.words.pop().is_some() {
        refresh_ranked(&mut s);
    }
}

pub fn on_space() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    finish_word(&mut s);
}

pub fn on_boundary() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    finish_word(&mut s);
}

fn finish_word(s: &mut PredictState) {
    if s.partial.len() >= 2 {
        let w = std::mem::take(&mut s.partial);
        record_completed(s, &w);
    }
    clear_strip(s);
}

/// Record a completed word: password-gated personal-dict learn (CONTEXT.md
/// "Secure field") + push to the VK-only context buffer (cap 8). Shared by
/// finish_word and Candidate commit so "word completed" lives in one place.
fn record_completed(s: &mut PredictState, word: &str) {
    maybe_learn(s, word);
    s.words.push(word.to_string());
    if s.words.len() > 8 {
        s.words.remove(0);
    }
}

/// Clear the partial + candidate strip after a word boundary or commit.
fn clear_strip(s: &mut PredictState) {
    s.partial.clear();
    s.ranked.clear();
    s.highlight = 0;
    s.candidate_engaged = false;
}

fn maybe_learn(s: &mut PredictState, word: &str) {
    let w = word.to_ascii_lowercase();
    if w.len() < 2 || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return;
    }
    match crate::win::logon_focus::focused_is_password_field() {
        Some(true) | None => return,
        Some(false) => {}
    }
    if s.personal.insert(w) {
        flush_personal(&s.personal);
    }
}

/// Commit the highlighted candidate through `sink` (CONTEXT.md "Candidate
/// commit"), but only when the user engaged the strip with LB/RB first. Deletes
/// the partial prefix and injects the chosen word as one Text-commit replace.
/// Returns the outcome, or None when there is nothing to commit. The engage gate
/// and the word pick share one lock, so a stale strip cannot commit. Personal-
/// dict learn + VK-buffer push happen only on a landed commit.
pub fn commit_if_engaged(
    sink: &mut dyn crate::vk_commit::TextSink,
) -> Option<crate::vk_commit::Committed> {
    let (word, del) = {
        let s = STATE.lock().ok()?;
        if !(s.candidate_engaged && strip_active_inner(&s)) {
            return None;
        }
        let idx = s.highlight.min(s.ranked.len() - 1);
        (s.ranked[idx].clone(), s.partial.chars().count())
    };
    let res = crate::vk_commit::commit(&word, del, sink);
    if let Ok(mut s) = STATE.lock() {
        if res.injected {
            record_completed(&mut s, &word);
        }
        clear_strip(&mut s);
    }
    Some(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn type_word(s: &str) {
        for c in s.chars() {
            on_char(c);
        }
        on_space();
    }

    #[test]
    fn prefix_finds_keyboard() {
        let _g = TEST_LOCK.lock().unwrap();
        reset();
        for c in "keyb".chars() {
            on_char(c);
        }
        assert!(strip().is_some());
        let ranked = STATE.lock().unwrap().ranked.clone();
        assert!(ranked.iter().any(|w| w == "keyboard"));
    }

    #[test]
    fn bigram_prefers_in_after_the() {
        let _g = TEST_LOCK.lock().unwrap();
        reset();
        type_word("the");
        on_char('i');
        on_char('n');
        let ranked = STATE.lock().unwrap().ranked.clone();
        assert!(!ranked.is_empty(), "ranked: {ranked:?}");
        assert_eq!(ranked[0], "in", "ranked: {ranked:?}");
    }

    #[test]
    fn viewport_at_end() {
        assert_eq!(viewport_start(4, 5), 2);
        assert_eq!(viewport_start(0, 5), 0);
    }

    #[test]
    fn a_does_not_commit_until_shoulder_cycle() {
        let _g = TEST_LOCK.lock().unwrap();
        reset();
        for c in "keyb".chars() {
            on_char(c);
        }
        assert!(strip().is_some());
        assert!(!strip().unwrap().engaged);
        let mut sink = crate::vk_commit::BufSink::new("keyb");
        assert!(commit_if_engaged(&mut sink).is_none());
        assert_eq!(sink.buf, "keyb"); // nothing injected
        assert!(cycle_next());
        assert!(strip().unwrap().engaged);
    }

    #[test]
    fn commit_replaces_prefix_with_word() {
        let _g = TEST_LOCK.lock().unwrap();
        reset();
        for c in "keyb".chars() {
            on_char(c);
        }
        cycle_next(); // engage the strip with LB/RB
        let highlighted = {
            let s = STATE.lock().unwrap();
            s.ranked[s.highlight].clone()
        };
        let mut sink = crate::vk_commit::BufSink::new("keyb");
        let res = commit_if_engaged(&mut sink).expect("engaged commit");
        assert!(res.injected);
        assert_eq!(res.deleted, 4);
        assert_eq!(sink.buf, highlighted);
        // landed commit records the word into the VK-only context buffer
        assert_eq!(STATE.lock().unwrap().words.last().unwrap(), &highlighted);
    }

    #[test]
    fn failed_inject_does_not_record() {
        let _g = TEST_LOCK.lock().unwrap();
        reset();
        for c in "keyb".chars() {
            on_char(c);
        }
        cycle_next();
        let mut sink = crate::vk_commit::BufSink::failing("keyb");
        let res = commit_if_engaged(&mut sink).expect("attempted commit");
        assert!(!res.injected);
        assert_eq!(sink.buf, "keyb"); // untouched
        assert!(STATE.lock().unwrap().words.is_empty()); // not recorded
    }
}
